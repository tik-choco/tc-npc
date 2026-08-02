// Turns the raw affect stream into the one `EmotionName` the avatar should
// currently wear, advancing the hysteresis-aware `decideEmotion` state
// machine (lib/vrm-emotion.ts) exactly once per *new* affect frame.
//
// Design choice: this computes the next decision eagerly, during render, by
// comparing the incoming `affect` against the last one seen (by reference)
// in a ref, rather than doing the update in a `useEffect` + `useState` pair.
//
//   - Correctness reason (why not effect+state): `decideEmotion` must be
//     called exactly once per distinct affect frame — call it twice for the
//     same frame and "held" (the hysteresis engine's "how many turns has
//     this expression been sitting" counter) overcounts, which is precisely
//     the double-state-machine bug this hook exists to avoid (see
//     AvatarView, which used to call the old `emotionFromAffect` twice, once
//     per render branch). A ref comparison done inline, before anything is
//     returned, is the only way to guarantee "one call per frame" that does
//     not depend on effect scheduling/dependency arrays running in a
//     particular order.
//   - Latency reason (why not effect+state): a `useEffect` runs *after* the
//     commit. With effect+state, the render that receives a new `affect`
//     would still paint the *previous* decision's emotion, the effect would
//     then queue a `setState`, and only the *next* render/paint would show
//     the updated expression — a full extra frame of lag on every affect
//     update, i.e. the face visibly trails the affect stream by one tick.
//     Computing the decision inline during render (and returning it
//     immediately) means the emotion shown is always the one for the
//     `affect` object being rendered right now, with no lag.
//
// Mutating a ref during render is unusual but sound here: it's idempotent
// for a given `affect` reference (re-rendering with the same `affect` object
// re-reads the same ref value without calling `decideEmotion` again), it
// never itself schedules a re-render, and the read/write only ever happens
// on this hook's own refs — no shared/module state, matching decideEmotion's
// own purity requirement.
import { useRef } from "preact/hooks";
import type { AffectSnapshot } from "./useNpcSocket";
import { decideEmotion, NEUTRAL_DECISION, type EmotionDecision } from "../lib/vrm-emotion";
import type { EmotionName } from "../vrm/animation";

/** Sentinel distinct from every legal `affect` value (including `null`,
 *  which means "no frame yet"), so the very first render always advances
 *  the decision once instead of being mistaken for "already processed". */
const UNSET: unique symbol = Symbol("useEmotion.unset");

export function useEmotion(affect: AffectSnapshot | null): EmotionName {
  const lastAffectRef = useRef<AffectSnapshot | null | typeof UNSET>(UNSET);
  const decisionRef = useRef<EmotionDecision>(NEUTRAL_DECISION);

  if (lastAffectRef.current !== affect) {
    lastAffectRef.current = affect;
    decisionRef.current = decideEmotion(affect, decisionRef.current);
  }

  return decisionRef.current.emotion;
}
