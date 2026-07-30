// Unit tests for emotionFromAffect, the vote-based mapping from the 22-drive
// affect snapshot onto a single VRM expression. The branching worth locking
// down: the neutral fallbacks (no frame, at-baseline, below-threshold), that
// only deviations past DEVIATION_THRESHOLD vote at all, that weighted votes
// for the same emotion accumulate, that a winning score must clear MIN_SCORE
// (0.28) and not merely equal it, and that the highest-scoring emotion wins
// even when more than one clears the bar.
import { describe, expect, it } from "vitest";
import type { AffectSnapshot } from "../hooks/useNpcSocket";
import { emotionFromAffect } from "./vrm-emotion";

function affectFrom(drives: AffectSnapshot["drives"]): AffectSnapshot {
  return {
    ts: 0,
    familiarity: 0,
    closing: false,
    inviteCaution: false,
    partner: null,
    partnerKnown: false,
    partnerSwitched: false,
    partnerAway: false,
    drives,
  };
}

describe("emotionFromAffect", () => {
  it("returns neutral when there is no affect frame yet", () => {
    expect(emotionFromAffect(null)).toBe("neutral");
  });

  it("returns neutral when every drive sits at its baseline", () => {
    const affect = affectFrom([
      { key: "dopamine", level: 0.5, base: 0.5 },
      { key: "cortisol", level: 0.3, base: 0.3 },
    ]);
    expect(emotionFromAffect(affect)).toBe("neutral");
  });

  it("ignores a deviation that hasn't crossed DEVIATION_THRESHOLD", () => {
    // dopamine up by 0.19 (< 0.2) never enters the vote at all.
    const affect = affectFrom([{ key: "dopamine", level: 0.69, base: 0.5 }]);
    expect(emotionFromAffect(affect)).toBe("neutral");
  });

  it("stays neutral when the winning score only ties MIN_SCORE (strict >)", () => {
    // cortisol up 0.28, weight 1 -> score exactly 0.28 == MIN_SCORE.
    const affect = affectFrom([{ key: "cortisol", level: 0.78, base: 0.5 }]);
    expect(emotionFromAffect(affect)).toBe("neutral");
  });

  it("picks happy once a strong reward-drive deviation clears MIN_SCORE", () => {
    // dopamine up 0.3, weight 1 -> score 0.3 > 0.28.
    const affect = affectFrom([{ key: "dopamine", level: 0.8, base: 0.5 }]);
    expect(emotionFromAffect(affect)).toBe("happy");
  });

  it("picks sad from cortisol rising above baseline", () => {
    const affect = affectFrom([{ key: "cortisol", level: 0.8, base: 0.5 }]); // delta 0.3
    expect(emotionFromAffect(affect)).toBe("sad");
  });

  it("picks angry from an adrenaline spike", () => {
    const affect = affectFrom([{ key: "adrenaline", level: 0.75, base: 0.5 }]); // delta 0.25, weight 1.2 -> 0.3
    expect(emotionFromAffect(affect)).toBe("angry");
  });

  it("picks relaxed from gaba rising above baseline", () => {
    const affect = affectFrom([{ key: "gaba", level: 0.85, base: 0.5 }]); // delta 0.35
    expect(emotionFromAffect(affect)).toBe("relaxed");
  });

  it("picks surprised from an acetylcholine spike", () => {
    const affect = affectFrom([{ key: "acetylcholine", level: 0.85, base: 0.5 }]); // delta 0.35, weight 0.9 -> 0.315
    expect(emotionFromAffect(affect)).toBe("surprised");
  });

  it("only counts a drive's vote in the direction it actually moved", () => {
    // dopamine has an "up" vote for happy (weight 1) and a "down" vote for
    // sad (weight 0.6); a drop of 0.3 should score sad 0.18 (below
    // MIN_SCORE) and must never be read as happy, even though the key
    // matches that vote too.
    const affect = affectFrom([{ key: "dopamine", level: 0.2, base: 0.5 }]); // delta -0.3
    expect(emotionFromAffect(affect)).toBe("neutral");
  });

  it("sums weighted votes from multiple drives backing the same emotion", () => {
    // Neither drive clears MIN_SCORE alone (0.2 and 0.15), but their sum
    // (0.35) for the same "happy" bucket does.
    const affect = affectFrom([
      { key: "endorphin", level: 0.75, base: 0.5 }, // delta 0.25 * weight 0.8 = 0.2
      { key: "anandamide", level: 0.75, base: 0.5 }, // delta 0.25 * weight 0.6 = 0.15
    ]);
    expect(emotionFromAffect(affect)).toBe("happy");
  });

  it("declares the highest-scoring emotion the winner when several clear the bar", () => {
    // happy: dopamine delta 0.5 * weight 1 = 0.5
    // angry: adrenaline delta 0.4 * weight 1.2 = 0.48
    // Both clear MIN_SCORE; happy has the larger score and should win.
    const affect = affectFrom([
      { key: "dopamine", level: 1, base: 0.5 },
      { key: "adrenaline", level: 0.9, base: 0.5 },
    ]);
    expect(emotionFromAffect(affect)).toBe("happy");
  });

  it("ignores drives that carry no vote at all", () => {
    const affect = affectFrom([{ key: "totally_unmapped_drive", level: 1, base: 0 }]);
    expect(emotionFromAffect(affect)).toBe("neutral");
  });
});
