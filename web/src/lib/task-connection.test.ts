// Unit tests for task-connection.ts's isTaskConfigured. The risky parts are
// the two independent ways a task can count as "configured" (a resolved
// preset, or a section's own base_url fallback) and that neither one being
// present correctly reads as "not configured".
import { describe, expect, it } from "vitest";
import type { ConfigDocument } from "./types";
import { isTaskConfigured } from "./task-connection";

describe("isTaskConfigured", () => {
  it("is false for a null config", () => {
    expect(isTaskConfigured(null, "vision")).toBe(false);
  });

  it("is false when the task has neither a preset nor its own base_url", () => {
    const config: ConfigDocument = { vision: {} };
    expect(isTaskConfigured(config, "vision")).toBe(false);
  });

  it("is true when the task's own preset_id resolves to a real preset", () => {
    const config: ConfigDocument = {
      providers: [{ id: "p1" }],
      presets: [{ id: "preset1", provider_id: "p1", model: "gpt-x" }],
      vision: { preset_id: "preset1" },
    };
    expect(isTaskConfigured(config, "vision")).toBe(true);
  });

  it("is true when preset_id is empty but default_preset_id resolves", () => {
    const config: ConfigDocument = {
      presets: [{ id: "preset1", model: "gpt-x" }],
      default_preset_id: "preset1",
      stt: { preset_id: "" },
    };
    expect(isTaskConfigured(config, "stt")).toBe(true);
  });

  it("is false when preset_id points at a preset that no longer exists", () => {
    const config: ConfigDocument = { presets: [], vision: { preset_id: "gone" } };
    expect(isTaskConfigured(config, "vision")).toBe(false);
  });

  it("falls back to the section's own base_url when no preset resolves", () => {
    const config: ConfigDocument = { stt: { base_url: "http://localhost:1234/v1" } };
    expect(isTaskConfigured(config, "stt")).toBe(true);
  });

  it("treats a blank base_url the same as unset", () => {
    const config: ConfigDocument = { vision: { base_url: "   " } };
    expect(isTaskConfigured(config, "vision")).toBe(false);
  });

  it("ignores a non-string base_url instead of throwing", () => {
    const config = { vision: { base_url: 123 } } as unknown as ConfigDocument;
    expect(isTaskConfigured(config, "vision")).toBe(false);
  });
});
