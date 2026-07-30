// three.js scene setup for a VRM avatar: lighting, a camera framed from the
// model's real (posed) proportions, and a user-drivable orbit around that
// framing.
//
// Ported from tc-town/src/vrm/stage.ts, minus its shared-renderer pool.
// tc-town renders a whole cast of bust avatars at once and has to fit them
// all through one WebGL context; tc-npc shows exactly one NPC, so the stage
// component owns a plain dedicated renderer (see components/VrmStage.tsx).

import * as THREE from "three";
import type { VRM } from "@pixiv/three-vrm";
import { createVrmAnimator, type VrmAnimator } from "./animation";

/** How the body is cropped: head-and-chest, head-to-hips, or head-to-toe. */
export type VrmFraming = "bust" | "upper" | "full";

/**
 * Serializable orbit state — everything a drag/zoom/pan gesture changes,
 * with no `THREE.Vector3`/live reference in it so it round-trips through
 * `JSON.stringify` for localStorage (see components/VrmStage.tsx). `pan` is
 * spelled out as plain numbers rather than kept as a Vector3 for the same
 * reason.
 */
export interface OrbitState {
  yaw: number;
  pitch: number;
  zoom: number;
  pan: { x: number; y: number; z: number };
}

export interface AvatarScene {
  scene: THREE.Scene;
  camera: THREE.PerspectiveCamera;
  vrm: VRM;
  animator: VrmAnimator;
  /** User-driven camera orbit around the framing look-at point. */
  orbit: {
    /** Apply a drag delta in radians. Yaw is unbounded (full turn allowed); pitch is clamped to roughly ±0.55 rad so the camera can't flip over. */
    rotateBy(dYaw: number, dPitch: number): void;
    /** Multiply the camera distance (wheel/pinch zoom); clamped so the camera can neither enter the model nor lose it in the distance. */
    zoomBy(factor: number): void;
    /** Pan the view in the camera's screen plane. dx/dy are screen-pixel deltas divided by the viewport height (drag right/down = positive); clamped so the model can't be panned fully out of frame. */
    panBy(dx: number, dy: number): void;
    /** Return to the initial front-on framing (angle, zoom, and pan). */
    reset(): void;
    /** Current angle/zoom/pan, for persisting across reloads. */
    snapshot(): OrbitState;
    /**
     * Restore a previously-snapshotted state. Goes through the exact same
     * clamps `rotateBy`/`zoomBy`/`panBy` apply (plus a finite-number check
     * on every field) so a value hand-edited in devtools, or one saved by an
     * older build with different limits, can't put the camera somewhere the
     * interactive controls never could — inside the model, past the pan
     * limit, flipped over the top, etc.
     */
    restore(state: OrbitState): void;
  };
}

/** Standard three-point-ish soft lighting used across avatar scenes. */
export function addAvatarLights(scene: THREE.Scene): void {
  scene.add(new THREE.AmbientLight(0xffffff, 0.9));
  const directional = new THREE.DirectionalLight(0xffffff, 1.2);
  directional.position.set(1, 1.5, 1);
  scene.add(directional);
  scene.add(new THREE.HemisphereLight(0xffffff, 0x8899aa, 0.35));
}

/**
 * Build an avatar scene around a VRM, framed as a head-and-chest "bust", a
 * waist-up "upper" shot, or a head-to-toe "full" body shot. In every case the
 * camera is solved from the model's real (posed) bounding box/bones so tall
 * hair, horns, etc. never get clipped by a fixed offset, and the required
 * camera distance is solved from both the vertical span *and* the horizontal
 * width so a portrait (taller-than-wide) `aspect` doesn't clip the sides even
 * though its horizontal FOV is narrower than its vertical FOV. The VRM's eyes
 * track the camera. `aspect` defaults to a square canvas and can be updated
 * later via the returned camera.
 *
 * The animator (which applies the arms-down standing pose to the normalized
 * humanoid bones) is created *before* any bone/box measurement below, and
 * `vrm.update(0)` propagates that pose to world matrices first — measuring
 * the raw T-pose here would frame the shot around the wrong shoulder width
 * and head height once the arms drop on the very next real frame.
 */
export function createAvatarScene(vrm: VRM, aspect = 1, framing: VrmFraming = "bust"): AvatarScene {
  const scene = new THREE.Scene();
  scene.background = null;
  addAvatarLights(scene);
  scene.add(vrm.scene);

  const camera = new THREE.PerspectiveCamera(29, aspect, 0.05, 20);

  const animator = createVrmAnimator(vrm, camera);
  vrm.update(0);
  vrm.scene.updateWorldMatrix(true, true);

  const head = vrm.humanoid?.getNormalizedBoneNode("head") ?? null;
  const chest =
    vrm.humanoid?.getNormalizedBoneNode("upperChest") ?? vrm.humanoid?.getNormalizedBoneNode("chest") ?? null;
  const hips = vrm.humanoid?.getNormalizedBoneNode("hips") ?? null;
  const leftUpperArm = vrm.humanoid?.getNormalizedBoneNode("leftUpperArm") ?? null;
  const rightUpperArm = vrm.humanoid?.getNormalizedBoneNode("rightUpperArm") ?? null;

  const box = new THREE.Box3().setFromObject(vrm.scene);
  const headPos = new THREE.Vector3();
  const chestPos = new THREE.Vector3();
  const hipsPos = new THREE.Vector3();

  if (head) {
    head.getWorldPosition(headPos);
  } else {
    box.getCenter(headPos);
    headPos.y = box.max.y - (box.max.y - box.min.y) * 0.12;
  }
  if (chest) {
    chest.getWorldPosition(chestPos);
  } else {
    // Fall back to a fixed offset below the head if the model has no chest bone.
    chestPos.set(headPos.x, headPos.y - 0.35, headPos.z);
  }
  if (hips) {
    hips.getWorldPosition(hipsPos);
  } else {
    hipsPos.set(chestPos.x, chestPos.y - 0.45, chestPos.z);
  }

  const vFovRad = THREE.MathUtils.degToRad(camera.fov);

  let topY: number;
  let bottomY: number;
  let lookY: number;
  let horizontalExtent: number;
  let horizontalMargin: number;

  if (framing === "full") {
    // Head-to-toe: use the posed bounding box's feet (min.y) and crown
    // (max.y) directly, plus a small top/bottom margin.
    const rawSpan = Math.max(0.5, box.max.y - box.min.y);
    const margin = rawSpan * 0.04;
    topY = box.max.y + margin;
    bottomY = box.min.y - margin;
    lookY = (box.max.y + box.min.y) / 2; // body center, not the head-biased bust lookY
    horizontalExtent = box.max.x - box.min.x;
    horizontalMargin = 1.25; // breathing room around the widest point of the (arms-down) body
  } else if (framing === "upper") {
    // Waist-up: same crown headroom as the bust, but frame down to just
    // below the hips so the face reads large while the torso still gives
    // the shot some presence.
    const crownY = Math.max(box.max.y, headPos.y + 0.1);
    topY = crownY + (crownY - headPos.y) * 0.2;
    bottomY = hipsPos.y - (chestPos.y - hipsPos.y) * 0.4;
    lookY = (topY + bottomY) / 2;
    horizontalExtent = box.max.x - box.min.x;
    horizontalMargin = 1.2; // arms-down body width already includes the arms; modest breathing room
  } else {
    // The head bone sits roughly at eye/jaw height, well below the crown of
    // the head (and further below any hair). Use the model's actual bounding
    // box for the crown instead of a fixed offset so tall hair/horns/hats on
    // unusual models still stay fully inside the frame.
    const crownY = Math.max(box.max.y, headPos.y + 0.1);
    topY = crownY + (crownY - headPos.y) * 0.2; // small headroom above the crown
    // Frame down past the chest bone toward the solar plexus (partway to the
    // hips) so the shot reads as head-to-chest, not a tight face crop.
    bottomY = chestPos.y - (chestPos.y - hipsPos.y) * 0.35;
    lookY = (topY + bottomY) / 2;

    // Shoulder width (bone-to-bone), falling back to the box width.
    if (leftUpperArm && rightUpperArm) {
      const l = new THREE.Vector3();
      const r = new THREE.Vector3();
      leftUpperArm.getWorldPosition(l);
      rightUpperArm.getWorldPosition(r);
      horizontalExtent = l.distanceTo(r);
    } else {
      horizontalExtent = box.max.x - box.min.x;
    }
    horizontalMargin = 1.4; // shoulder joints sit inside the body's visual width; leave room for it plus breathing space
  }

  const verticalSpan = Math.max(0.35, topY - bottomY);
  const VERTICAL_MARGIN = 1.15; // headroom so the crop doesn't touch the top/bottom edges
  const distanceForHeight = (verticalSpan * VERTICAL_MARGIN) / (2 * Math.tan(vFovRad / 2));

  // `camera.fov` is the *vertical* FOV; the horizontal FOV shrinks with a
  // portrait `aspect` (< 1). Solve for the distance that keeps the body
  // inside that narrower horizontal frame too, and use whichever distance
  // (height- or width-driven) is larger so nothing gets clipped.
  const halfWidth = (Math.max(horizontalExtent, 0.28) * horizontalMargin) / 2;
  const distanceForWidth = halfWidth / (Math.tan(vFovRad / 2) * aspect);

  const distance = Math.max(distanceForHeight, distanceForWidth);

  camera.position.set(headPos.x, lookY, headPos.z + distance);
  camera.lookAt(headPos.x, lookY, headPos.z);

  // Eyes follow the avatar camera for a lifelike gaze.
  if (vrm.lookAt) vrm.lookAt.target = camera;

  // The orbit pivots around the same look-at point and radius the framing
  // math above solved for, so dragging never changes how tightly the body is
  // cropped — only the viewing angle around it. Yaw/pitch start at 0, which
  // by construction reproduces the exact position/lookAt set above.
  const pivot = new THREE.Vector3(headPos.x, lookY, headPos.z);
  const orbitRadius = distance;
  let yaw = 0;
  let pitch = 0;
  let zoom = 1;
  const PITCH_LIMIT = 0.55;
  // Zoom is a multiplier on the solved framing distance: 0.4 gets close to a
  // face shot without the near plane (0.05) clipping into the head, 2.5 keeps
  // the model comfortably inside the far plane (20).
  const ZOOM_MIN = 0.4;
  const ZOOM_MAX = 2.5;
  // Pan shifts the look-at point in the camera's screen plane; capped to a
  // fraction of the framing distance so the model can't leave the frame.
  const panOffset = new THREE.Vector3();
  const PAN_LIMIT = orbitRadius * 0.6;
  const panRight = new THREE.Vector3();
  const panForward = new THREE.Vector3();
  const panUp = new THREE.Vector3();
  const lookTarget = new THREE.Vector3();

  const applyOrbit = (): void => {
    const radius = orbitRadius * zoom;
    lookTarget.copy(pivot).add(panOffset);
    camera.position.set(
      lookTarget.x + radius * Math.sin(yaw) * Math.cos(pitch),
      lookTarget.y + radius * Math.sin(pitch),
      lookTarget.z + radius * Math.cos(yaw) * Math.cos(pitch),
    );
    camera.lookAt(lookTarget);
  };

  const orbit = {
    rotateBy(dYaw: number, dPitch: number): void {
      yaw += dYaw;
      pitch = THREE.MathUtils.clamp(pitch + dPitch, -PITCH_LIMIT, PITCH_LIMIT);
      applyOrbit();
    },
    zoomBy(factor: number): void {
      zoom = THREE.MathUtils.clamp(zoom * factor, ZOOM_MIN, ZOOM_MAX);
      applyOrbit();
    },
    panBy(dx: number, dy: number): void {
      // Scale normalized screen deltas to world units at the pivot's depth so
      // the content tracks the cursor 1:1 regardless of zoom level.
      const worldHeight = 2 * Math.tan(vFovRad / 2) * orbitRadius * zoom;
      panRight.set(Math.cos(yaw), 0, -Math.sin(yaw));
      panForward.set(-Math.sin(yaw) * Math.cos(pitch), -Math.sin(pitch), -Math.cos(yaw) * Math.cos(pitch));
      panUp.crossVectors(panRight, panForward);
      // Drag right/down moves the character with the cursor = camera slides
      // the opposite way horizontally, the same way vertically (screen y is
      // inverted relative to world up).
      panOffset.addScaledVector(panRight, -dx * worldHeight * camera.aspect);
      panOffset.addScaledVector(panUp, dy * worldHeight);
      if (panOffset.length() > PAN_LIMIT) panOffset.setLength(PAN_LIMIT);
      applyOrbit();
    },
    reset(): void {
      yaw = 0;
      pitch = 0;
      zoom = 1;
      panOffset.set(0, 0, 0);
      applyOrbit();
    },
    snapshot(): OrbitState {
      return { yaw, pitch, zoom, pan: { x: panOffset.x, y: panOffset.y, z: panOffset.z } };
    },
    restore(state: OrbitState): void {
      // Yaw is unbounded in normal use too (a full turn is legal), but still
      // needs a finite check: a corrupt stored value shouldn't leave `yaw`
      // as NaN, which would poison every subsequent rotateBy() delta added
      // to it forever.
      yaw = Number.isFinite(state?.yaw) ? state.yaw : 0;
      pitch = THREE.MathUtils.clamp(Number.isFinite(state?.pitch) ? state.pitch : 0, -PITCH_LIMIT, PITCH_LIMIT);
      zoom = THREE.MathUtils.clamp(Number.isFinite(state?.zoom) ? state.zoom : 1, ZOOM_MIN, ZOOM_MAX);
      const pan = state?.pan;
      panOffset.set(
        Number.isFinite(pan?.x) ? pan.x : 0,
        Number.isFinite(pan?.y) ? pan.y : 0,
        Number.isFinite(pan?.z) ? pan.z : 0,
      );
      if (panOffset.length() > PAN_LIMIT) panOffset.setLength(PAN_LIMIT);
      applyOrbit();
    },
  };

  return { scene, camera, vrm, animator, orbit };
}
