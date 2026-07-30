// Unit tests for the shared affect-model presentation helpers: which drives
// count as "deviating" (rankDeviations), which way a delta is styled
// (deltaDirection), and the 0..1 -> 0..100 display clamp (clampPct). These
// three feed both BrainView's per-drive rows and StatusPanel's condensed
// 内心 block, so their edge cases (empty snapshot, at-baseline drives,
// boundary deviations) are exactly what keeps those two views in sync.
import { describe, expect, it } from "vitest";
import type { AffectSnapshot } from "../hooks/useNpcSocket";
import { clampPct, deltaDirection, rankDeviations } from "./affect";

function snapshot(drives: AffectSnapshot["drives"]): AffectSnapshot {
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

describe("clampPct", () => {
  it("scales a 0..1 fraction to a 0..100 percentage", () => {
    expect(clampPct(0.5)).toBe(50);
    expect(clampPct(0)).toBe(0);
    expect(clampPct(1)).toBe(100);
  });

  it("clamps values below 0 up to 0", () => {
    expect(clampPct(-0.3)).toBe(0);
    expect(clampPct(-100)).toBe(0);
  });

  it("clamps values above 1 down to 100", () => {
    expect(clampPct(1.5)).toBe(100);
    expect(clampPct(50)).toBe(100);
  });

  it("rounds to the nearest whole percentage", () => {
    expect(clampPct(0.126)).toBe(13);
    expect(clampPct(0.124)).toBe(12);
  });
});

describe("deltaDirection", () => {
  it("reads a clearly positive delta as up", () => {
    expect(deltaDirection(0.001)).toBe("up");
    expect(deltaDirection(1)).toBe("up");
  });

  it("reads a clearly negative delta as down", () => {
    expect(deltaDirection(-0.001)).toBe("down");
    expect(deltaDirection(-1)).toBe("down");
  });

  it("treats zero as flat", () => {
    expect(deltaDirection(0)).toBe("flat");
  });

  it("keeps a rounding-error-sized delta flat rather than flickering up/down", () => {
    expect(deltaDirection(0.0003)).toBe("flat");
    expect(deltaDirection(-0.0003)).toBe("flat");
  });

  it("treats the dead-zone boundary itself as flat (strict inequality)", () => {
    expect(deltaDirection(0.0005)).toBe("flat");
    expect(deltaDirection(-0.0005)).toBe("flat");
  });
});

describe("rankDeviations", () => {
  it("returns an empty list for a null snapshot", () => {
    expect(rankDeviations(null)).toEqual([]);
  });

  it("returns an empty list when every drive sits at its baseline", () => {
    const affect = snapshot([
      { key: "dopamine", level: 0.5, base: 0.5 },
      { key: "cortisol", level: 0.2, base: 0.2 },
    ]);
    expect(rankDeviations(affect)).toEqual([]);
  });

  it("excludes a drive deviating just under the threshold", () => {
    const affect = snapshot([{ key: "dopamine", level: 0.69, base: 0.5 }]); // |delta| = 0.19
    expect(rankDeviations(affect)).toEqual([]);
  });

  it("includes a drive deviating at exactly the threshold", () => {
    // base 0 rather than e.g. 0.5 so the subtraction lands on exactly 0.2 in
    // floating point instead of 0.19999999999999998.
    const affect = snapshot([{ key: "dopamine", level: 0.2, base: 0 }]); // |delta| = 0.2
    const result = rankDeviations(affect);
    expect(result).toHaveLength(1);
    expect(result[0]).toEqual({
      key: "dopamine",
      level: 0.2,
      base: 0,
      delta: 0.2,
      deviation: 0.2,
    });
  });

  it("keeps the sign of the delta while ranking by absolute deviation", () => {
    const affect = snapshot([
      { key: "serotonin", level: 0.2, base: 0.5 }, // delta -0.3
      { key: "dopamine", level: 0.75, base: 0.5 }, // delta +0.25
    ]);
    const result = rankDeviations(affect);
    expect(result.map((d) => d.key)).toEqual(["serotonin", "dopamine"]);
    expect(result[0].delta).toBe(-0.3);
    expect(result[1].delta).toBeCloseTo(0.25);
  });

  it("sorts strongest deviation first regardless of input order", () => {
    const affect = snapshot([
      { key: "a", level: 0.75, base: 0.5 }, // 0.25
      { key: "b", level: 0.95, base: 0.5 }, // 0.45
      { key: "c", level: 0.71, base: 0.5 }, // 0.21
    ]);
    expect(rankDeviations(affect).map((d) => d.key)).toEqual(["b", "a", "c"]);
  });
});
