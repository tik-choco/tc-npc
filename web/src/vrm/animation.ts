// Cheap idle + speaking animation for a VRM: standing pose (arms down instead
// of the raw T-pose), auto-blink, subtle breathing/sway, a mouth that tracks
// the NPC's actual voice loudness while audible (falling back to a canned
// sine cadence if no loudness reading is available — see the mouth block in
// update() below), and emotion-driven facial expressions. All effects only
// touch bones/expressions that actually exist on the model, so it is safe on
// any VRM. One VrmAnimator is created per VRM instance; call update() each
// frame BEFORE vrm.update(delta) so expression weights/pose are applied that
// frame.
//
// Ported from tc-town/src/vrm/animation.ts (itself carrying the standing pose
// + idle sway from tc-vrm-viewer's idleMotion.ts). The emotion set is the
// VRM 1.0 standard expression set, which is what both apps drive.

import * as THREE from "three";
import type { VRM } from "@pixiv/three-vrm";
import type { SpeakingLevelReading } from "./level";

/** The VRM standard expression presets this animator can drive. */
export type EmotionName = "neutral" | "happy" | "angry" | "sad" | "relaxed" | "surprised";

/** Preferred mouth expression names, in order (VRM1 aa / VRM0 A). */
const MOUTH_CANDIDATES = ["aa", "a", "A", "ih", "ou"];

/** Drops the T-pose arms to the sides (radians, ~80deg). */
const ARM_DOWN_ANGLE = 1.4;

/** Preferred expression names per emotion, in order. */
const EMOTION_CANDIDATES: Record<Exclude<EmotionName, "neutral">, string[]> = {
  happy: ["happy", "joy", "fun"],
  angry: ["angry"],
  sad: ["sad", "sorrow"],
  relaxed: ["relaxed"],
  surprised: ["surprised", "surprise"],
};

/** Weight change per second while easing an emotion expression in/out. */
const EMOTION_EASE_RATE = 6;

// --- Level-driven mouth (see SpeakingLevelMessage in lib/types.ts). ---
//
// `speakingLevel` frames land roughly every 50ms; this animator's update()
// runs once per rendered frame (~60Hz), so most ticks see the exact same
// reading the previous tick did. "Is there a live feed?" is therefore
// answered by the reading's `seq` (see vrm/level.ts), never by comparing
// `level` — silence and a clamped-loud passage both send genuine runs of
// identical values, and treating those as a stall would flap the mouth
// through the very silences it should stay shut for.
//
// LEVEL_STALE_SECONDS is how long we go without a new `seq` before deciding
// there is no feed at all (the server predates this frame, or it stalled
// mid-line) and falling back to the fixed sine cadence, rather than leaving
// the mouth frozen at its last opening for the rest of the line. 5x the
// nominal cadence absorbs ordinary jitter (a slow tick, a delayed frame)
// without waiting long enough for a genuinely stuck feed to read as a frozen
// mouth for multiple words.
const LEVEL_FRAME_INTERVAL = 0.05;
const LEVEL_STALE_SECONDS = LEVEL_FRAME_INTERVAL * 5;

// Envelope-follower rates for the level-driven mouth, tuned like a
// compressor: attack is faster than release, so the mouth snaps open on a
// vowel/plosive onset but eases shut a touch slower. A single symmetric
// smoothing rate reads noticeably more robotic on percussive speech than
// this asymmetric one does.
const LEVEL_ATTACK_RATE = 22;
const LEVEL_RELEASE_RATE = 10;

// Raw loudness spends most of a sentence in the low-mid range with only
// occasional peaks, so mapping it straight to openness leaves the mouth
// looking like it barely moves. A gamma < 1 lifts the mid-range into a
// livelier-looking openness while both ends of the range still hit fully
// closed/open.
const LEVEL_GAMMA = 0.7;

// A little high-frequency flutter layered on top of the level-driven
// openness so it doesn't read as a linear volume meter mapped onto a jaw —
// scaled by the current openness so it never invents motion out of silence.
const LEVEL_WOBBLE_AMOUNT = 0.12;
const LEVEL_WOBBLE_RATE = 24;

// Head gaze-follow: how far the neck is allowed to turn toward the camera
// before it just holds the limit (the body never turns with it), and how
// quickly it eases toward the target orientation each frame. The damping
// factor is time-based (1 - e^-k*dt) so the follow speed is frame-rate
// independent and settles smoothly instead of snapping when the camera
// orbits quickly.
const GAZE_YAW_LIMIT = 0.6;
const GAZE_PITCH_LIMIT = 0.35;
const GAZE_DAMPING_RATE = 8;

/** Wraps an angle to (-π, π] so relative yaw deltas near the ±π seam don't spin the long way around. */
function wrapAngle(rad: number): number {
  return THREE.MathUtils.euclideanModulo(rad + Math.PI, Math.PI * 2) - Math.PI;
}

export interface VrmAnimator {
  /**
   * Advance idle + speaking + emotion animation. Call each frame before
   * vrm.update(). `level` is the latest loudness reading (meaningless while
   * `speaking` is false); `emotion` is the NPC's current expression (or
   * null/"neutral" for none).
   */
  update(
    deltaSeconds: number,
    speaking: boolean,
    level: SpeakingLevelReading,
    emotion?: EmotionName | null,
  ): void;
}

export function createVrmAnimator(vrm: VRM, lookAtTarget?: THREE.Object3D): VrmAnimator {
  const expressionNames = new Set(
    (vrm.expressionManager?.expressions ?? [])
      .map((expression) => expression.expressionName)
      .filter((name): name is string => Boolean(name)),
  );
  const hasBlink = expressionNames.has("blink");
  const mouthExpression = MOUTH_CANDIDATES.find((name) => expressionNames.has(name));

  // Pick the first available expression name per emotion, skipping anything
  // already claimed by blink/mouth so the channels never fight each other.
  const emotionExpression = new Map<EmotionName, string>();
  for (const emotion of Object.keys(EMOTION_CANDIDATES) as Array<Exclude<EmotionName, "neutral">>) {
    const name = EMOTION_CANDIDATES[emotion].find(
      (candidate) => expressionNames.has(candidate) && candidate !== "blink" && candidate !== mouthExpression,
    );
    if (name) emotionExpression.set(emotion, name);
  }
  const emotionWeights = new Map<EmotionName, number>();
  for (const emotion of emotionExpression.keys()) emotionWeights.set(emotion, 0);

  // --- Standing pose (arms down instead of T-pose) + idle sway bones. ---
  const humanoid = vrm.humanoid;
  const leftUpperArm = humanoid?.getNormalizedBoneNode("leftUpperArm") ?? null;
  const rightUpperArm = humanoid?.getNormalizedBoneNode("rightUpperArm") ?? null;
  const leftLowerArm = humanoid?.getNormalizedBoneNode("leftLowerArm") ?? null;
  const rightLowerArm = humanoid?.getNormalizedBoneNode("rightLowerArm") ?? null;
  // VRM0/1 rigs can bake a 180° residual into the rest pose (see rotateVRM0
  // in loader.ts), which flips which Z sign actually points the arm down —
  // derive it from the lowerArm rest position instead of assuming a fixed sign.
  const leftArmSign = leftLowerArm && leftLowerArm.position.x !== 0 ? -Math.sign(leftLowerArm.position.x) : 1;
  const rightArmSign = rightLowerArm && rightLowerArm.position.x !== 0 ? -Math.sign(rightLowerArm.position.x) : -1;
  // Breathing bone: prefer upperChest, falling back to chest, then spine.
  const breathBone =
    humanoid?.getNormalizedBoneNode("upperChest") ??
    humanoid?.getNormalizedBoneNode("chest") ??
    humanoid?.getNormalizedBoneNode("spine") ??
    null;
  const spineBone = humanoid?.getNormalizedBoneNode("spine") ?? null;
  const headBone = humanoid?.getNormalizedBoneNode("head") ?? null;

  const breathBaseX = breathBone ? breathBone.rotation.x : 0;
  const spineBaseZ = spineBone ? spineBone.rotation.z : 0;
  const headBaseX = headBone ? headBone.rotation.x : 0;
  const headBaseY = headBone ? headBone.rotation.y : 0;

  // Apply the arms-down stance immediately so the model never renders (even
  // for one frame) in its raw T-pose.
  leftUpperArm?.rotation.set(0, 0, leftArmSign * ARM_DOWN_ANGLE);
  rightUpperArm?.rotation.set(0, 0, rightArmSign * ARM_DOWN_ANGLE);

  let blinkTimer = randomBlinkInterval();
  let blinkElapsed = 0;
  let blinkPhase: "idle" | "closing" | "opening" = "idle";
  let blinkPhaseElapsed = 0;

  let idleElapsed = 0;
  let mouthElapsed = 0;
  let mouthWeight = 0;
  // Envelope-followed openness derived from the loudness readings, plus the
  // `seq` of the last one seen. `-1` is not a legal seq (it counts up from
  // 0), so it reliably reads as "no reading has arrived yet".
  let smoothedLevel = 0;
  let lastLevelSeq = -1;
  let timeSinceLevel = Number.POSITIVE_INFINITY;

  // Gaze-follow state: current damped yaw/pitch offset applied on top of the
  // idle head rotation, plus scratch objects reused every frame (this runs
  // in the render loop, so no per-frame allocations).
  let gazeYaw = 0;
  let gazePitch = 0;
  // Parent-space forward differs between VRM0/VRM1 rigs (the importer may
  // bake a 180° yaw into the hierarchy), so "straight ahead" can't be assumed
  // to be +Z. The camera starts front-on by construction, so the first
  // measured head→target direction IS the neutral gaze — capture it once and
  // measure every later frame relative to it.
  let gazeRefYaw: number | null = null;
  let gazeRefPitch = 0;
  // VRMUtils.rotateVRM0 (loader.ts) bakes a 180° yaw into vrm.scene for VRM0
  // models AFTER the humanoid's normalized rig is built, so the world-rotation
  // snapshots three-vrm's VRMHumanoidRig.update() composes with were captured
  // pre-flip. The rendered head forward then comes out as the full negation of
  // the "+Z forward" the rotation.x/y writes below assume. Negating a vector
  // shifts yaw by 180° (absorbed by the gazeRef baseline) but flips the SIGN
  // of pitch, which no baseline subtraction can absorb — so on VRM0 rigs the
  // pitch offset must be applied mirrored. The measured yaw/pitch below stay
  // correct either way (a pure Y rotation never changes atan2(-y, hypot(x,z))).
  const gazePitchApplySign = vrm.meta?.metaVersion === "0" ? -1 : 1;
  const gazeHeadWorldPos = new THREE.Vector3();
  const gazeTargetWorldPos = new THREE.Vector3();
  const gazeDirection = new THREE.Vector3();
  const gazeParentWorldQuat = new THREE.Quaternion();

  const setExpr = (name: string | undefined, weight: number) => {
    if (name) vrm.expressionManager?.setValue(name, weight);
  };

  return {
    update(deltaSeconds, speaking, level, emotion) {
      // --- Standing pose: keep the arms-down base rotation every frame. ---
      leftUpperArm?.rotation.set(0, 0, leftArmSign * ARM_DOWN_ANGLE);
      rightUpperArm?.rotation.set(0, 0, rightArmSign * ARM_DOWN_ANGLE);

      // --- Idle sway: tiny sinusoidal breathing/chest/spine/head motion,
      // layered on top of the standing pose's base rotations. ---
      idleElapsed += deltaSeconds;
      if (breathBone) {
        breathBone.rotation.x = breathBaseX + Math.sin(idleElapsed * 1.4) * 0.02;
      }
      if (spineBone) {
        spineBone.rotation.z = spineBaseZ + Math.sin(idleElapsed * 0.55) * 0.015;
      }
      if (headBone) {
        headBone.rotation.y = headBaseY + Math.sin(idleElapsed * 0.3) * 0.05;
        headBone.rotation.x = headBaseX + Math.sin(idleElapsed * 0.45 + 1.5) * 0.025;
      }

      // --- Head gaze-follow: turn the head (not the body) softly toward
      // lookAtTarget (typically the orbiting camera), composed ON TOP of the
      // idle sway rotation set just above so the two never fight. Skipped
      // entirely when no target was supplied or the model has no head bone. ---
      if (lookAtTarget && headBone) {
        headBone.getWorldPosition(gazeHeadWorldPos);
        lookAtTarget.getWorldPosition(gazeTargetWorldPos);
        gazeDirection.subVectors(gazeTargetWorldPos, gazeHeadWorldPos);
        // Normalized humanoid bones share a consistent rest orientation
        // (local +Z = forward) regardless of VRM0/1 source, so express the
        // target direction in the head's parent space to get yaw/pitch
        // relative to that forward axis.
        const headParent = headBone.parent;
        if (headParent) {
          headParent.getWorldQuaternion(gazeParentWorldQuat);
          gazeDirection.applyQuaternion(gazeParentWorldQuat.invert());
        }
        if (gazeDirection.lengthSq() > 1e-8) {
          gazeDirection.normalize();
          const horizontal = Math.hypot(gazeDirection.x, gazeDirection.z);
          const rawYaw = Math.atan2(gazeDirection.x, gazeDirection.z);
          const rawPitch = Math.atan2(-gazeDirection.y, horizontal);
          if (gazeRefYaw === null) {
            gazeRefYaw = rawYaw;
            gazeRefPitch = rawPitch;
          }
          const desiredYaw = THREE.MathUtils.clamp(wrapAngle(rawYaw - gazeRefYaw), -GAZE_YAW_LIMIT, GAZE_YAW_LIMIT);
          const desiredPitch = THREE.MathUtils.clamp(
            rawPitch - gazeRefPitch,
            -GAZE_PITCH_LIMIT,
            GAZE_PITCH_LIMIT,
          );
          // Time-based damping so orbiting the camera produces a natural
          // delayed follow instead of the head snapping to the new angle.
          const followFactor = 1 - Math.exp(-GAZE_DAMPING_RATE * deltaSeconds);
          gazeYaw += (desiredYaw - gazeYaw) * followFactor;
          gazePitch += (desiredPitch - gazePitch) * followFactor;
          headBone.rotation.y += gazeYaw;
          headBone.rotation.x += gazePitchApplySign * gazePitch;
        }
      }

      // --- Auto-blink. ---
      if (hasBlink) {
        blinkElapsed += deltaSeconds;
        if (blinkPhase === "idle") {
          if (blinkElapsed >= blinkTimer) {
            blinkPhase = "closing";
            blinkPhaseElapsed = 0;
            blinkElapsed = 0;
          }
        } else {
          blinkPhaseElapsed += deltaSeconds;
          if (blinkPhase === "closing") {
            const weight = Math.min(1, blinkPhaseElapsed / 0.08);
            setExpr("blink", weight);
            if (weight >= 1) {
              blinkPhase = "opening";
              blinkPhaseElapsed = 0;
            }
          } else {
            const weight = Math.max(0, 1 - blinkPhaseElapsed / 0.12);
            setExpr("blink", weight);
            if (weight <= 0) {
              blinkPhase = "idle";
              blinkTimer = randomBlinkInterval();
            }
          }
        }
      }

      // --- Mouth movement while speaking. ---
      if (mouthExpression) {
        if (speaking) {
          mouthElapsed += deltaSeconds;

          // Guard against a malformed/out-of-range frame before it can poison
          // the envelope follower below (a NaN, once mixed in via the
          // exponential blend, never recovers on its own).
          const safeLevel = Number.isFinite(level.level) ? THREE.MathUtils.clamp(level.level, 0, 1) : 0;

          // Liveness comes from `seq`, which counts readings rather than
          // describing them, so a run of identical loudnesses still reads as
          // a live feed. Silence is exactly such a run — and it is the case
          // that matters most, since falling back there would animate a
          // mouth through a pause.
          if (level.seq !== lastLevelSeq) {
            lastLevelSeq = level.seq;
            timeSinceLevel = 0;
          } else {
            timeSinceLevel += deltaSeconds;
          }

          if (timeSinceLevel <= LEVEL_STALE_SECONDS) {
            // Track the level's envelope with asymmetric attack/release
            // (see LEVEL_ATTACK_RATE/LEVEL_RELEASE_RATE above), then reshape
            // it and add a subtle proportional wobble so it reads as a mouth
            // rather than a meter needle (see LEVEL_GAMMA/LEVEL_WOBBLE_*).
            const rate = safeLevel > smoothedLevel ? LEVEL_ATTACK_RATE : LEVEL_RELEASE_RATE;
            smoothedLevel += (safeLevel - smoothedLevel) * (1 - Math.exp(-rate * deltaSeconds));
            const shaped = Math.pow(THREE.MathUtils.clamp(smoothedLevel, 0, 1), LEVEL_GAMMA);
            const wobble = shaped * LEVEL_WOBBLE_AMOUNT * Math.sin(mouthElapsed * LEVEL_WOBBLE_RATE);
            mouthWeight = THREE.MathUtils.clamp(shaped + wobble, 0, 1);
          } else {
            // No live level in the last LEVEL_STALE_SECONDS: either the
            // server predates `speakingLevel` or the feed has stalled mid-
            // line. Fall back to the original fixed two-sine cadence rather
            // than leaving the mouth frozen at whatever it last showed.
            const target = 0.5 + 0.35 * Math.sin(mouthElapsed * 17) + 0.15 * Math.sin(mouthElapsed * 5.3);
            mouthWeight = THREE.MathUtils.clamp(mouthWeight + (target - mouthWeight) * 0.5, 0, 1);
          }
        } else {
          mouthWeight = Math.max(0, mouthWeight - deltaSeconds * 6);
          // Each speaking bout starts by assuming no level has arrived yet
          // (rather than inheriting the previous bout's reading), so a new
          // line falls back to the sine cadence until its own
          // `speakingLevel` frames actually land instead of momentarily
          // reusing a stale envelope from the line before it.
          smoothedLevel = 0;
          lastLevelSeq = -1;
          timeSinceLevel = Number.POSITIVE_INFINITY;
        }
        setExpr(mouthExpression, mouthWeight);
      }

      // --- Emotion expression: ease the active emotion's weight toward 1
      // and every other tracked emotion toward 0 (fade-out/fade-in). ---
      if (emotionExpression.size > 0) {
        const activeEmotion = emotion && emotion !== "neutral" && emotionExpression.has(emotion) ? emotion : null;
        const maxDelta = deltaSeconds * EMOTION_EASE_RATE;
        for (const [name, exprName] of emotionExpression) {
          const target = name === activeEmotion ? 1 : 0;
          const current = emotionWeights.get(name) ?? 0;
          const next = moveToward(current, target, maxDelta);
          emotionWeights.set(name, next);
          setExpr(exprName, next);
        }
      }
    },
  };
}

function moveToward(current: number, target: number, maxDelta: number): number {
  if (current < target) return Math.min(current + maxDelta, target);
  if (current > target) return Math.max(current - maxDelta, target);
  return current;
}

function randomBlinkInterval(): number {
  return 2 + Math.random() * 3;
}
