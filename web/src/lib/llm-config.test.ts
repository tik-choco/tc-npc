// Unit tests for the provider/preset config model shared by the AI settings
// UI. The risky parts are the defensive reads (a config document is
// untyped JSON that may be missing sections, non-array, or have the wrong
// field types), the "" = follow default_preset_id resolution chain, and
// pruneDanglingPresetRefs, which must reset every reference to a
// provider/preset that just got deleted rather than leaving the config
// pointing at something that no longer exists.
import { describe, expect, it } from "vitest";
import type { ConfigDocument } from "./types";
import type { PresetEntry, ProviderEntry } from "./config-types";
import {
  addPreset,
  addProvider,
  deletePreset,
  deleteProvider,
  newId,
  presetBadgeKeys,
  presetCountForProvider,
  presetLabel,
  providerLabel,
  providerOf,
  readDefaultPresetId,
  readPresets,
  readProviders,
  resolvePreset,
  resolveTaskPreset,
  setDefaultPreset,
  setTaskPreset,
  taskPresetId,
  updatePreset,
  updateProvider,
  type LlmTaskId,
  type Mutate,
} from "./llm-config";

/** Mimics useConfigDoc's contract: `mutate` hands the updater a deep-cloned
 *  draft and the result becomes the new document, without the test needing
 *  a real config-doc hook or a server round-trip. */
function harness(initial: ConfigDocument): { mutate: Mutate; get: () => ConfigDocument } {
  let current = initial;
  const mutate: Mutate = (updater) => {
    const draft = structuredClone(current);
    updater(draft);
    current = draft;
  };
  return { mutate, get: () => current };
}

const provider1: ProviderEntry = { id: "prov1", label: "Local", base_url: "http://localhost:1234" };
const provider2: ProviderEntry = { id: "prov2", label: "Cloud" };
const preset1: PresetEntry = { id: "preset1", label: "Chat model", provider_id: "prov1", model: "gpt-x" };
const preset2: PresetEntry = { id: "preset2", provider_id: "prov2", model: "gpt-y" };

describe("readProviders / readPresets / readDefaultPresetId", () => {
  it("returns empty defaults for a null config", () => {
    expect(readProviders(null)).toEqual([]);
    expect(readPresets(null)).toEqual([]);
    expect(readDefaultPresetId(null)).toBe("");
  });

  it("returns empty defaults when the field is present but not the right shape", () => {
    const config: ConfigDocument = { providers: "nope", presets: 42, default_preset_id: 7 };
    expect(readProviders(config)).toEqual([]);
    expect(readPresets(config)).toEqual([]);
    expect(readDefaultPresetId(config)).toBe("");
  });

  it("passes through well-formed arrays/strings", () => {
    const config: ConfigDocument = {
      providers: [provider1],
      presets: [preset1],
      default_preset_id: "preset1",
    };
    expect(readProviders(config)).toEqual([provider1]);
    expect(readPresets(config)).toEqual([preset1]);
    expect(readDefaultPresetId(config)).toBe("preset1");
  });
});

describe("resolvePreset", () => {
  const config: ConfigDocument = {
    presets: [preset1, preset2],
    default_preset_id: "preset2",
  };

  it("resolves a preset by its own id", () => {
    expect(resolvePreset(config, "preset1")).toEqual(preset1);
  });

  it("falls back to default_preset_id when presetId is empty", () => {
    expect(resolvePreset(config, "")).toEqual(preset2);
  });

  it("returns null when neither the id nor the default resolve to anything", () => {
    expect(resolvePreset({}, "")).toBeNull();
  });

  it("returns null for a dangling id that matches no preset", () => {
    expect(resolvePreset(config, "no-such-preset")).toBeNull();
  });

  it("returns null when default_preset_id itself is dangling", () => {
    const dangling: ConfigDocument = { presets: [preset1], default_preset_id: "ghost" };
    expect(resolvePreset(dangling, "")).toBeNull();
  });
});

describe("providerOf", () => {
  const config: ConfigDocument = { providers: [provider1, provider2] };

  it("returns null for a null preset", () => {
    expect(providerOf(config, null)).toBeNull();
  });

  it("returns null when the preset has no provider_id", () => {
    expect(providerOf(config, { id: "p", model: "m" })).toBeNull();
  });

  it("returns null when provider_id doesn't match any provider", () => {
    expect(providerOf(config, { id: "p", provider_id: "ghost" })).toBeNull();
  });

  it("finds the matching provider", () => {
    expect(providerOf(config, preset1)).toEqual(provider1);
  });
});

describe("taskPresetId / resolveTaskPreset", () => {
  it("reads a task's own preset_id from its section", () => {
    const config: ConfigDocument = { talk: { preset_id: "preset1" } };
    expect(taskPresetId(config, "talk")).toBe("preset1");
  });

  it("reads the embedding task from memory.embedding_preset_id, a different field on the same section", () => {
    const config: ConfigDocument = {
      memory: { preset_id: "preset1", embedding_preset_id: "preset2" },
    };
    expect(taskPresetId(config, "memory")).toBe("preset1");
    expect(taskPresetId(config, "embedding")).toBe("preset2");
  });

  it("defaults to empty when the section is missing or the field isn't a string", () => {
    expect(taskPresetId(null, "talk")).toBe("");
    expect(taskPresetId({ talk: { preset_id: 42 } }, "talk")).toBe("");
  });

  it("defaults to empty for an unrecognized task id", () => {
    expect(taskPresetId({}, "bogus-task" as LlmTaskId)).toBe("");
  });

  it("resolves a task with no assignment to the default preset", () => {
    const config: ConfigDocument = {
      presets: [preset1],
      default_preset_id: "preset1",
      talk: { preset_id: "" },
    };
    expect(resolveTaskPreset(config, "talk")).toEqual(preset1);
  });

  it("resolves a task with its own assignment over the default", () => {
    const config: ConfigDocument = {
      presets: [preset1, preset2],
      default_preset_id: "preset2",
      talk: { preset_id: "preset1" },
    };
    expect(resolveTaskPreset(config, "talk")).toEqual(preset1);
  });
});

describe("presetLabel / providerLabel", () => {
  it("prefers the preset's own label", () => {
    expect(presetLabel({ id: "x", label: "Custom", model: "m" })).toBe("Custom");
  });

  it("falls back to the model name when the label is empty or whitespace", () => {
    expect(presetLabel({ id: "x", label: "  ", model: "gpt-x" })).toBe("gpt-x");
    expect(presetLabel({ id: "x", model: "gpt-x" })).toBe("gpt-x");
  });

  it("falls back to the id when both label and model are absent", () => {
    expect(presetLabel({ id: "x" })).toBe("x");
  });

  it("mirrors the same fallback chain for providers (label, then id)", () => {
    expect(providerLabel({ id: "p", label: "Local" })).toBe("Local");
    expect(providerLabel({ id: "p", label: "   " })).toBe("p");
    expect(providerLabel({ id: "p" })).toBe("p");
  });
});

describe("newId", () => {
  it("starts at 1 for an empty existing list", () => {
    expect(newId("preset", [])).toBe("preset1");
  });

  it("picks the smallest unused N, not one past the count", () => {
    expect(newId("preset", ["preset2"])).toBe("preset1");
  });

  it("skips every N already taken, even non-contiguously", () => {
    expect(newId("preset", ["preset1", "preset2", "preset4"])).toBe("preset3");
  });

  it("ignores ids that don't share the prefix", () => {
    expect(newId("preset", ["provider1", "other"])).toBe("preset1");
  });
});

describe("presetBadgeKeys", () => {
  it("badges the default preset, but only when presetId is non-empty", () => {
    const config: ConfigDocument = { default_preset_id: "" };
    expect(presetBadgeKeys(config, "")).toEqual([]);
  });

  it("badges a preset that is the exact default_preset_id", () => {
    const config: ConfigDocument = { default_preset_id: "preset1" };
    expect(presetBadgeKeys(config, "preset1")).toContain("llm.badge.default");
  });

  it("badges a preset with tasks directly assigned to it", () => {
    // resolveTaskPreset only matches a real preset object, so the presets
    // array must actually contain "preset1" for the assignment to resolve.
    const config: ConfigDocument = {
      presets: [preset1],
      default_preset_id: "",
      talk: { preset_id: "preset1" },
      vision: { preset_id: "preset1" },
    };
    const keys = presetBadgeKeys(config, "preset1");
    expect(keys).toContain("task.talk");
    expect(keys).toContain("task.vision");
    expect(keys).not.toContain("llm.badge.default");
  });

  it("also badges tasks that reach this preset only by following the default", () => {
    const config: ConfigDocument = {
      presets: [preset1],
      default_preset_id: "preset1",
      talk: { preset_id: "" }, // follows default
    };
    const keys = presetBadgeKeys(config, "preset1");
    expect(keys).toContain("llm.badge.default");
    expect(keys).toContain("task.talk");
  });
});

describe("presetCountForProvider", () => {
  it("counts zero for a provider with no presets or a missing presets array", () => {
    expect(presetCountForProvider(null, "prov1")).toBe(0);
    expect(presetCountForProvider({ presets: [preset2] }, "prov1")).toBe(0);
  });

  it("counts every preset bound to the provider", () => {
    const config: ConfigDocument = { presets: [preset1, preset2, { id: "preset3", provider_id: "prov1" }] };
    expect(presetCountForProvider(config, "prov1")).toBe(2);
  });
});

describe("provider/preset mutation helpers", () => {
  it("addProvider appends to an empty or missing providers array", () => {
    const h = harness({});
    addProvider(h.mutate, provider1);
    expect(readProviders(h.get())).toEqual([provider1]);
  });

  it("updateProvider patches only the matching id", () => {
    const h = harness({ providers: [provider1, provider2] });
    updateProvider(h.mutate, "prov1", { label: "Renamed" });
    expect(readProviders(h.get())).toEqual([{ ...provider1, label: "Renamed" }, provider2]);
  });

  it("addPreset appends to an empty or missing presets array", () => {
    const h = harness({});
    addPreset(h.mutate, preset1);
    expect(readPresets(h.get())).toEqual([preset1]);
  });

  it("updatePreset patches only the matching id", () => {
    const h = harness({ presets: [preset1, preset2] });
    updatePreset(h.mutate, "preset1", { label: "Renamed" });
    expect(readPresets(h.get())).toEqual([{ ...preset1, label: "Renamed" }, preset2]);
  });

  it("setDefaultPreset overwrites default_preset_id unconditionally", () => {
    const h = harness({ default_preset_id: "preset1" });
    setDefaultPreset(h.mutate, "preset2");
    expect(readDefaultPresetId(h.get())).toBe("preset2");
  });

  it("setTaskPreset writes into the task's own section/field, preserving sibling fields", () => {
    const h = harness({ memory: { enabled: true, preset_id: "preset1" } });
    setTaskPreset(h.mutate, "embedding", "preset2");
    expect(h.get().memory).toEqual({ enabled: true, preset_id: "preset1", embedding_preset_id: "preset2" });
  });

  it("setTaskPreset does nothing for an unrecognized task id", () => {
    const h = harness({ talk: { preset_id: "preset1" } });
    setTaskPreset(h.mutate, "bogus-task" as LlmTaskId, "preset2");
    expect(h.get().talk).toEqual({ preset_id: "preset1" });
  });

  it("deletePreset clears default_preset_id and any task assignment pointing at it", () => {
    const h = harness({
      presets: [preset1, preset2],
      default_preset_id: "preset1",
      talk: { preset_id: "preset1" },
      vision: { preset_id: "preset2" },
    });
    deletePreset(h.mutate, "preset1");
    const config = h.get();
    expect(readPresets(config)).toEqual([preset2]);
    expect(readDefaultPresetId(config)).toBe(""); // was preset1, now dangling
    expect(taskPresetId(config, "talk")).toBe(""); // was preset1, now dangling
    expect(taskPresetId(config, "vision")).toBe("preset2"); // untouched
  });

  it("deleteProvider also removes its presets and prunes references left dangling by that", () => {
    const h = harness({
      providers: [provider1, provider2],
      presets: [preset1, preset2],
      default_preset_id: "preset1", // bound to provider1's preset
      talk: { preset_id: "preset1" },
      vision: { preset_id: "preset2" }, // bound to provider2 - must survive
    });
    deleteProvider(h.mutate, "prov1");
    const config = h.get();
    expect(readProviders(config)).toEqual([provider2]);
    expect(readPresets(config)).toEqual([preset2]);
    expect(readDefaultPresetId(config)).toBe("");
    expect(taskPresetId(config, "talk")).toBe("");
    expect(taskPresetId(config, "vision")).toBe("preset2");
  });
});
