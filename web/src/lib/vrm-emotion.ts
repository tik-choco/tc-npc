// Turning the affect model into a face.
//
// tc-town classifies each reply's emotion with a separate LLM call. tc-npc
// doesn't need to: it already computes a 22-drive affect state every turn
// (npc-talk's `PartnerAffect`, arriving as the `affect` frame), which is a
// richer and cheaper signal than asking a model to label the text. This maps
// that state onto the six VRM standard expressions the animator can drive.
//
// The mapping is deliberately conservative — it reads the drives that are
// *deviating from their own baseline*, the same measure the affect engine
// uses to decide what reaches the persona prompt (see lib/affect.ts), so the
// face only changes when the NPC's internal state actually moved. At
// baseline it returns "neutral" and the model just idles.

import type { AffectSnapshot } from "../hooks/useNpcSocket";
import type { EmotionName } from "../vrm/animation";
import { DEVIATION_THRESHOLD } from "./affect";

/**
 * Which drives, deviating in which direction, argue for which expression.
 *
 * `weight` scales a drive's deviation when it votes, so a drive that is a
 * strong signal for an emotion (oxytocin for happiness, cortisol for
 * distress) outweighs a supporting one. A drive may appear under several
 * emotions; the strongest total vote wins.
 */
const VOTES: Array<{ key: string; direction: "up" | "down"; emotion: Exclude<EmotionName, "neutral">; weight: number }> = [
  // Reward/bonding above baseline reads as pleased.
  { key: "dopamine", direction: "up", emotion: "happy", weight: 1 },
  { key: "oxytocin", direction: "up", emotion: "happy", weight: 1.2 },
  { key: "endorphin", direction: "up", emotion: "happy", weight: 0.8 },
  { key: "anandamide", direction: "up", emotion: "happy", weight: 0.6 },

  // Stress/threat above baseline reads as upset. Adrenaline separates
  // "angry" from the flatter "sad": a spike is confrontation, its absence
  // is closer to being worn down.
  { key: "adrenaline", direction: "up", emotion: "angry", weight: 1.2 },
  { key: "noradrenaline", direction: "up", emotion: "angry", weight: 0.8 },
  { key: "substance_p", direction: "up", emotion: "angry", weight: 0.5 },

  { key: "cortisol", direction: "up", emotion: "sad", weight: 1 },
  { key: "dynorphin", direction: "up", emotion: "sad", weight: 1 },
  { key: "serotonin", direction: "down", emotion: "sad", weight: 0.9 },
  { key: "dopamine", direction: "down", emotion: "sad", weight: 0.6 },

  // Calm: the inhibitory/settled drives up, without the stress ones.
  { key: "gaba", direction: "up", emotion: "relaxed", weight: 1 },
  { key: "serotonin", direction: "up", emotion: "relaxed", weight: 0.9 },
  { key: "melatonin", direction: "up", emotion: "relaxed", weight: 0.7 },
  { key: "npy", direction: "up", emotion: "relaxed", weight: 0.5 },

  // Sudden arousal/alertness without the threat drives reads as surprise.
  { key: "acetylcholine", direction: "up", emotion: "surprised", weight: 0.9 },
  { key: "histamine", direction: "up", emotion: "surprised", weight: 0.7 },
  { key: "orexin", direction: "up", emotion: "surprised", weight: 0.5 },
];

/**
 * How much total vote an emotion needs before it is worn at all. Roughly
 * "one clearly-deviating strong drive, or two mild ones" — high enough that
 * ordinary conversational drift leaves the face neutral rather than making
 * the model pull expressions at every turn.
 */
const MIN_SCORE = 0.28;

/**
 * The expression the NPC should be wearing, given its latest affect frame.
 * Returns `"neutral"` for no frame, a frame sitting at baseline, or a
 * winning emotion too weak to be worth showing.
 */
export function emotionFromAffect(affect: AffectSnapshot | null): EmotionName {
  if (!affect) return "neutral";

  const scores = new Map<EmotionName, number>();
  for (const drive of affect.drives) {
    const delta = drive.level - drive.base;
    if (Math.abs(delta) < DEVIATION_THRESHOLD) continue;
    for (const vote of VOTES) {
      if (vote.key !== drive.key) continue;
      if (vote.direction === "up" ? delta <= 0 : delta >= 0) continue;
      scores.set(vote.emotion, (scores.get(vote.emotion) ?? 0) + Math.abs(delta) * vote.weight);
    }
  }

  let best: EmotionName = "neutral";
  let bestScore = MIN_SCORE;
  for (const [emotion, score] of scores) {
    if (score > bestScore) {
      best = emotion;
      bestScore = score;
    }
  }
  return best;
}
