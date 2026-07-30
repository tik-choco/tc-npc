// Unit test for the reasoning-effort option list: the Rust side
// (crates/npc-core) is gaining an "xhigh" step alongside
// none/minimal/low/medium/high, and the UI's segmented control needs to
// offer the same set or a saved "xhigh" value would render as a blank pick.
import { describe, expect, it } from "vitest";
import { PRESET_EFFORT_OPTIONS, REASONING_EFFORT_OPTIONS } from "./SettingsFields";

describe("REASONING_EFFORT_OPTIONS", () => {
  it("includes xhigh as the step above high", () => {
    const values = REASONING_EFFORT_OPTIONS.map((o) => o.value);
    expect(values).toEqual(["none", "minimal", "low", "medium", "high", "xhigh"]);
  });

  it("gives xhigh its own translated hint key rather than reusing high's", () => {
    const xhigh = REASONING_EFFORT_OPTIONS.find((o) => o.value === "xhigh");
    expect(xhigh?.hintKey).toBe("settings.effort.xhigh");
  });
});

describe("PRESET_EFFORT_OPTIONS", () => {
  it("carries xhigh through in addition to the leading inherit option", () => {
    const values = PRESET_EFFORT_OPTIONS.map((o) => o.value);
    expect(values).toEqual(["", "none", "minimal", "low", "medium", "high", "xhigh"]);
  });
});
