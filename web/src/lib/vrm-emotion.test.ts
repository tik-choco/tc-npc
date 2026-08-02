// Unit tests for decideEmotion, the stateful (per-conversation hysteresis)
// replacement for the old stateless emotionFromAffect.
//
// Most of the old test file is gone rather than ported, because most of it
// was pinning behaviour this rewrite deliberately changes:
//   - "returns neutral ... below MIN_SCORE" tests assumed neutral is a
//     fallback with no score of its own. It now has an explicit score that
//     competes in the same argmax (see computeScores / NEUTRAL_BASE); the
//     replacement here is "neutral wins over a real vote", not "everything
//     defaults to neutral".
//   - The MIN_SCORE=0.28-tuned boundary tests (cortisol delta exactly 0.28,
//     etc.) pinned raw, non-normalized deviations. Every drive's deviation
//     is normalized against its own headroom now (see normalizedDeviation),
//     so those exact boundary numbers no longer mean anything.
//   - The "dopamine up -> happy" test voted a bare dopamine deviation
//     straight into happy. dopamine is exactly the kind of
//     length/question-shaped arousal drive this rewrite demotes to a
//     modifier that cannot originate a score on its own (see MODIFIER_VOTES)
//     — that old test's premise is the bug being fixed, not a behaviour to
//     keep.
// What's still true and still tested here: a deviation below the floor
// never votes, votes for the same emotion from different drives sum, and
// the highest-scoring emotion wins. What's new and now covered: neutral
// competing rather than falling back to, normalization treating different
// bases fairly, arousal-only drives needing an existing valence vote to
// attach to, the wariness/invite-caution corrections, the full six-key
// `scores` contract, and decideEmotion's hysteresis (switch margin, and
// what MIN_HOLD_FRAMES's current tuned value of 1 does and doesn't cover —
// see that constant's doc comment in vrm-emotion.ts).
//
// Also covers the re-tuning round's CORTISOL_WARINESS_CORRECTION: cortisol
// now only gets 70% of warinessBias's estimate subtracted (not the full
// amount noradrenaline still gets) — see the "wariness and invite-caution
// cancellation" tests below, and CORTISOL_WARINESS_CORRECTION's own doc
// comment in vrm-emotion.ts for the dev-split measurements behind 0.7.
import { describe, expect, it } from "vitest";
import type { AffectSnapshot } from "../hooks/useNpcSocket";
import type { EmotionName } from "../vrm/animation";
import { decideEmotion, NEUTRAL_DECISION, type EmotionDecision } from "./vrm-emotion";

const ALL_EMOTIONS: readonly EmotionName[] = ["neutral", "happy", "angry", "sad", "relaxed", "surprised"];

function affectFrom(
  drives: AffectSnapshot["drives"],
  overrides: Partial<Omit<AffectSnapshot, "drives">> = {},
): AffectSnapshot {
  return {
    ts: 0,
    familiarity: 1, // fully familiar by default, so wariness/invite-caution
    // corrections are inert unless a test opts into them explicitly.
    closing: false,
    inviteCaution: false,
    partner: null,
    partnerKnown: false,
    partnerSwitched: false,
    partnerAway: false,
    drives,
    ...overrides,
  };
}

describe("decideEmotion", () => {
  describe("no affect / no signal", () => {
    it("returns neutral (held 1) when there is no affect frame and no history", () => {
      const decision = decideEmotion(null, null);
      expect(decision.emotion).toBe("neutral");
      expect(decision.held).toBe(1);
    });

    it("returns neutral when every supplied drive sits exactly at its baseline", () => {
      const affect = affectFrom([
        { key: "dopamine", level: 0.5, base: 0.5 },
        { key: "cortisol", level: 0.3, base: 0.3 },
      ]);
      expect(decideEmotion(affect, null).emotion).toBe("neutral");
    });

    it("ignores a deviation that hasn't cleared the normalized floor", () => {
      // endorphin (base 0.35, weight 1.0) needs normalized deviation >= 0.2,
      // i.e. delta >= 0.2 * (1 - 0.35) = 0.13. 0.10 stays under that.
      const affect = affectFrom([{ key: "endorphin", level: 0.45, base: 0.35 }]);
      expect(decideEmotion(affect, null).emotion).toBe("neutral");
    });
  });

  describe("scores always carries all six keys", () => {
    it("for a null affect frame", () => {
      const decision = decideEmotion(null, null);
      expect(Object.keys(decision.scores).sort()).toEqual([...ALL_EMOTIONS].sort());
    });

    it("for a populated affect frame, including emotions nothing voted for", () => {
      const affect = affectFrom([{ key: "endorphin", level: 0.9, base: 0.35 }]);
      const decision = decideEmotion(affect, null);
      expect(Object.keys(decision.scores).sort()).toEqual([...ALL_EMOTIONS].sort());
      // angry/sad/relaxed/surprised had nothing arguing for them this frame
      // — still present, at 0, not omitted.
      expect(decision.scores.angry).toBe(0);
      expect(decision.scores.sad).toBe(0);
      expect(decision.scores.relaxed).toBe(0);
      expect(decision.scores.surprised).toBe(0);
    });
  });

  describe("neutral competes on its own score rather than being a fallback", () => {
    it("beats a lone vote that only just cleared the floor", () => {
      // dhea: base 0.5, weight 0.6. level 0.625 -> delta 0.125 -> normalized
      // deviation 0.25, comfortably past the 0.2 floor but still a single
      // mild vote. happy score = 0.6 * 0.25 = 0.15.
      // neutral = 0.3 - 0.5 * 0.15 = 0.225.
      // A single mild, floor-clearing signal is exactly the case old
      // MIN_SCORE-style thresholding would have let win outright once past
      // the bar; here neutral still wins comfortably.
      const affect = affectFrom([{ key: "dhea", level: 0.625, base: 0.5 }]);
      const decision = decideEmotion(affect, null);
      expect(decision.scores.happy).toBeCloseTo(0.15, 5);
      expect(decision.scores.neutral).toBeCloseTo(0.225, 5);
      expect(decision.emotion).toBe("neutral");
    });

    it("loses once the same drive's deviation is strong enough", () => {
      // Same dhea, pushed to normalized deviation 0.8 (delta 0.4, level 0.9).
      // happy score = 0.6 * 0.8 = 0.48. neutral = 0.3 - 0.5 * 0.48 = 0.06.
      const affect = affectFrom([{ key: "dhea", level: 0.9, base: 0.5 }]);
      const decision = decideEmotion(affect, null);
      expect(decision.scores.happy).toBeCloseTo(0.48, 5);
      expect(decision.scores.neutral).toBeCloseTo(0.06, 5);
      expect(decision.emotion).toBe("happy");
    });
  });

  describe("normalization compares drives with different baselines fairly", () => {
    it("scores two fully-saturated drives equally despite very different raw deltas", () => {
      // endorphin: base 0.35, headroom to 1.0 is 0.65, weight 1.0.
      // gaba: base 0.5, headroom to 1.0 is 0.5, weight 1.0.
      // Both clamped to level 1.0 -> normalized deviation 1.0 for both, even
      // though the raw deltas (0.65 vs 0.5) differ. A raw-delta comparison
      // (the old design) would have scored endorphin higher for reaching
      // further in absolute terms; normalizing treats "used all of my own
      // range" the same regardless of how big that range was.
      const affect = affectFrom([
        { key: "endorphin", level: 1.0, base: 0.35 },
        { key: "gaba", level: 1.0, base: 0.5 },
      ]);
      const decision = decideEmotion(affect, null);
      expect(decision.scores.happy).toBeCloseTo(1.0, 5);
      expect(decision.scores.relaxed).toBeCloseTo(1.0, 5);
      expect(decision.scores.happy).toBeCloseTo(decision.scores.relaxed, 5);
    });
  });

  describe("arousal-shaped drives can only sharpen a real vote, never originate one", () => {
    it("a large dopamine deviation alone contributes nothing to happy", () => {
      // dopamine: base 0.5, normalized deviation (0.9-0.5)/(1-0.5) = 0.8 —
      // large — but happy has no primary vote on the board this frame, so
      // the MODIFIER_VOTES gate (scores.happy > 0) never opens.
      const affect = affectFrom([{ key: "dopamine", level: 0.9, base: 0.5 }]);
      const decision = decideEmotion(affect, null);
      expect(decision.scores.happy).toBe(0);
      expect(decision.emotion).toBe("neutral");
    });

    it("the same dopamine deviation sharpens an already-real happy vote", () => {
      // endorphin barely past the floor (see the neutral test above: score
      // 0.12) plus the same dopamine deviation as above. dopamine's modifier
      // weight is 0.25, so it adds 0.25 * 0.8 = 0.2 once the gate is open.
      const affect = affectFrom([
        { key: "endorphin", level: 0.48, base: 0.35 },
        { key: "dopamine", level: 0.9, base: 0.5 },
      ]);
      const decision = decideEmotion(affect, null);
      // endorphin alone: (0.48-0.35)/0.65 = 0.2 -> 1.0 * 0.2 = 0.2 primary.
      expect(decision.scores.happy).toBeCloseTo(0.2 + 0.25 * 0.8, 5);
      expect(decision.emotion).toBe("happy");
    });
  });

  describe("wariness and invite-caution cancellation", () => {
    it("cancels a cortisol reading that is fully explained by low-familiarity wariness", () => {
      // At familiarity 0, warinessBias() = (0.45 - max(0, 0-0.15)) * 0.5 =
      // 0.225. Cortisol only gets CORTISOL_WARINESS_CORRECTION (0.7, i.e.
      // 70%) of that subtracted — see its doc comment for why a full
      // subtraction is deliberately not used — so a level built from
      // *exactly* the wariness estimate (0.3 + 0.225 = 0.525) doesn't land
      // on precisely zero after correction: 0.7 * 0.225 = 0.1575 is
      // subtracted, leaving a raw residual of 0.525 - 0.3 - 0.1575 =
      // 0.0675, normalized 0.0675 / 0.7 ≈ 0.096. That's still comfortably
      // under DEVIATION_FLOOR (0.2), so it doesn't clear the bar to vote at
      // all — "a total stranger" alone, no distressing content, still
      // correctly reads as not-sad, just via "too small to vote" rather
      // than "corrected to exactly zero".
      const affect = affectFrom([{ key: "cortisol", level: 0.525, base: 0.3 }], { familiarity: 0 });
      const decision = decideEmotion(affect, null);
      expect(decision.scores.sad).toBe(0);
      expect(decision.emotion).toBe("neutral");
    });

    it("the same cortisol reading reads as real sadness once familiarity removes the wariness confound", () => {
      // familiarity 0.6 is past the 0.45 wariness cutoff, so wariness is
      // exactly 0 and none of the same delta is subtracted.
      const affect = affectFrom([{ key: "cortisol", level: 0.525, base: 0.3 }], { familiarity: 0.6 });
      const decision = decideEmotion(affect, null);
      expect(decision.scores.sad).toBeGreaterThan(0);
      expect(decision.emotion).toBe("sad");
    });

    it("only partially corrects a cortisol reading that mixes wariness with genuine content", () => {
      // Same familiarity-0 wariness estimate (0.225) as above, but this
      // level (0.63) carries more than just wariness: raw delta 0.33 versus
      // wariness's 0.225, i.e. ~0.105 of real content on top. This is
      // exactly the case CORTISOL_WARINESS_CORRECTION being less than 1.0
      // exists for (see its doc comment): a *full* subtraction would leave
      // 0.33 - 0.225 = 0.105, normalized 0.105 / 0.7 = 0.15 — still under
      // DEVIATION_FLOOR, so the genuine content would be silently erased
      // along with the wariness confound, exactly the "sad reads as dead"
      // failure the re-tuning round found. The actual 70% correction
      // leaves 0.33 - 0.7 * 0.225 = 0.1725, normalized 0.1725 / 0.7 ≈
      // 0.2464, clearing the floor — the genuine content survives. Cortisol's
      // PRIMARY_VOTES weight (0.7) happens to equal its headroom (1 - base =
      // 0.7) here, so the score collapses to exactly the remaining raw
      // delta: 0.2464 * 0.7 = 0.1725.
      const affect = affectFrom([{ key: "cortisol", level: 0.63, base: 0.3 }], { familiarity: 0 });
      const decision = decideEmotion(affect, null);
      expect(decision.scores.sad).toBeCloseTo(0.1725, 4);
      expect(decision.scores.sad).toBeGreaterThan(0);
    });

    it("invite-caution's flat cck bonus does not by itself read as anger", () => {
      // cck's bias correction while invite-caution is active is a flat 0.2
      // (see inviteCautionBias). A cck reading built from exactly that
      // bonus, with no independent conflict/pain content behind it, cancels
      // out the same way the wariness case above does.
      const affect = affectFrom([{ key: "cck", level: 0.45, base: 0.25 }], { inviteCaution: true });
      const decision = decideEmotion(affect, null);
      expect(decision.scores.angry).toBe(0);
    });

    it("a cck reading well past the invite-caution bonus still needs the raised floor to fire", () => {
      // level 0.6375: post-bias normalized deviation is 0.25 — comfortably
      // past cck's ordinary floor (0.12) but under INVITE_CAUTION_CCK_FLOOR
      // (0.35), so it still doesn't vote while caution is active...
      const cautious = affectFrom([{ key: "cck", level: 0.6375, base: 0.25 }], { inviteCaution: true });
      expect(decideEmotion(cautious, null).scores.angry).toBe(0);

      // ...but the identical raw level reads as a clear angry vote once
      // there's no invite-caution correction to apply at all.
      const ordinary = affectFrom([{ key: "cck", level: 0.6375, base: 0.25 }], { inviteCaution: false });
      const decision = decideEmotion(ordinary, null);
      expect(decision.scores.angry).toBeGreaterThan(0.3);
      expect(decision.emotion).toBe("angry");
    });
  });

  describe("hysteresis", () => {
    // gaba alone: normalized deviation 0.4 -> relaxed score 1.0 * 0.4 = 0.4.
    const gabaOnly = affectFrom([{ key: "gaba", level: 0.7, base: 0.5 }]);

    it("previous=null and NEUTRAL_DECISION behave identically on the first frame", () => {
      const fromNull = decideEmotion(gabaOnly, null);
      const fromSentinel = decideEmotion(gabaOnly, NEUTRAL_DECISION);
      expect(fromNull.emotion).toBe(fromSentinel.emotion);
      expect(fromNull.held).toBe(fromSentinel.held);
      expect(fromNull.scores).toEqual(fromSentinel.scores);
      expect(fromNull.emotion).toBe("relaxed");
      expect(fromNull.held).toBe(1);
    });

    it("holds the current expression when a challenger doesn't clear SWITCH_MARGIN", () => {
      const first = decideEmotion(gabaOnly, null);
      expect(first.emotion).toBe("relaxed");

      // Same gaba (relaxed still scores 0.4) plus endorphin normalized 0.5
      // -> happy score 0.5. 0.5 is the frame's argmax (0.5 > 0.4) but only
      // beats relaxed's 0.4 by 0.1, under the 0.13 SWITCH_MARGIN.
      const challenge = affectFrom([
        { key: "gaba", level: 0.7, base: 0.5 },
        { key: "endorphin", level: 0.675, base: 0.35 },
      ]);
      const second = decideEmotion(challenge, first);
      expect(second.scores.happy).toBeCloseTo(0.5, 5);
      expect(second.scores.relaxed).toBeCloseTo(0.4, 5);
      expect(second.emotion).toBe("relaxed");
      expect(second.held).toBe(first.held + 1);
    });

    it("switches once a challenger clears SWITCH_MARGIN over the held expression's current score", () => {
      const first = decideEmotion(gabaOnly, null);
      const blocked = decideEmotion(
        affectFrom([
          { key: "gaba", level: 0.7, base: 0.5 },
          { key: "endorphin", level: 0.675, base: 0.35 },
        ]),
        first,
      );
      expect(blocked.emotion).toBe("relaxed"); // sanity check, see previous test

      // Same gaba (relaxed 0.4) plus a stronger endorphin: normalized 0.6 ->
      // happy score 0.6, which clears relaxed's 0.4 + the 0.13 margin.
      const stronger = affectFrom([
        { key: "gaba", level: 0.7, base: 0.5 },
        { key: "endorphin", level: 0.74, base: 0.35 },
      ]);
      const third: EmotionDecision = decideEmotion(stronger, blocked);
      expect(third.scores.happy).toBeCloseTo(0.6, 5);
      expect(third.emotion).toBe("happy");
      // held resets on a switch, rather than continuing to accumulate.
      expect(third.held).toBe(1);
    });

    it("keeps incrementing held while the argmax genuinely agrees with the current expression", () => {
      const first = decideEmotion(gabaOnly, null);
      const second = decideEmotion(gabaOnly, first);
      const fourth = decideEmotion(gabaOnly, second);
      expect(second.emotion).toBe("relaxed");
      expect(second.held).toBe(2);
      expect(fourth.emotion).toBe("relaxed");
      expect(fourth.held).toBe(3);
    });
  });

  describe("weighted votes for the same emotion still sum", () => {
    it("combines two drives that individually clear the floor", () => {
      // endorphin: (0.6-0.35)/0.65 = 0.3846... * 1.0
      // anandamide: (0.6-0.4)/0.6 = 0.3333... * 0.8
      const affect = affectFrom([
        { key: "endorphin", level: 0.6, base: 0.35 },
        { key: "anandamide", level: 0.6, base: 0.4 },
      ]);
      const decision = decideEmotion(affect, null);
      const expected = ((0.6 - 0.35) / 0.65) * 1.0 + ((0.6 - 0.4) / 0.6) * 0.8;
      expect(decision.scores.happy).toBeCloseTo(expected, 5);
      expect(decision.emotion).toBe("happy");
    });
  });

  describe("surprised needs a much larger deviation than the other expressions", () => {
    it("an ordinary acetylcholine deviation does not read as surprised", () => {
      // normalized deviation 0.3 would clear the ordinary DEVIATION_FLOOR
      // (0.2) if that applied, but acetylcholine's surprised vote uses the
      // much higher SURPRISED_FLOOR (0.5) instead, which 0.3 doesn't clear.
      const affect = affectFrom([{ key: "acetylcholine", level: 0.615, base: 0.45 }]);
      expect(decideEmotion(affect, null).scores.surprised).toBe(0);
    });

    it("a near-saturated acetylcholine deviation does read as surprised", () => {
      const affect = affectFrom([{ key: "acetylcholine", level: 0.98, base: 0.45 }]);
      const decision = decideEmotion(affect, null);
      expect(decision.scores.surprised).toBeGreaterThan(0);
      expect(decision.emotion).toBe("surprised");
    });
  });

  it("ignores a drive key that carries no vote at all", () => {
    const affect = affectFrom([{ key: "totally_unmapped_drive", level: 1, base: 0 }]);
    expect(decideEmotion(affect, null).emotion).toBe("neutral");
  });
});
