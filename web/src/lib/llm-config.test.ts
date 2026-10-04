import { describe, expect, it } from "vitest";
import { deleteProvider, migrateSharedLlmConfig, resolveModel } from "@tik-choco/mistai/llm-config";
import { addRoom, migrateConfigDocument, readProviders, resolveTaskModel, setTaskRef, sharedConfig, taskRef, writeShared } from "./llm-config";
import type { ConfigDocument } from "./types";

const legacy = (): ConfigDocument => ({
  providers: [{ id: "http", label: "Endpoint", base_url: "http://example.test/v1", api_key: "***", enabled: true, models: Array.from({ length: 300 }, (_, i) => `model-${i}`), untouched: 42 },
    { id: "disabled", base_url: "http://disabled.test/v1", enabled: false, models: ["private"] }],
  presets: [{ id: "old", provider_id: "http", model: "manual", reasoning_effort: "high" }],
  default_preset_id: "old", talk: { preset_id: "old", enabled: true },
  memory: { preset_id: "old", reasoning_effort: "medium", embedding_preset_id: "old", untouched: true },
  mist: { room_id: "team", signaling_url: "wss://example.test" },
  networkProviderEnabled: true, sharedPresetIds: ["old"], unrelated: { keep: true },
});
describe("REST ModelRef adapter", () => {
  it("does not repeatedly save Rust default sections with an empty room ID", () => {
    expect(migrateConfigDocument({ providers: [], mist: { room_id: "" } })).toBe(false);
  });
  it("ignores retired room mirror presets instead of synthesizing an assignment", () => {
    const doc: ConfigDocument = { providers: [{ id: "room", base_url: "mist-network://team" }], presets: [{ id: "mirror", provider_id: "room", model: "remote" }], talk: { preset_id: "mirror" } };
    migrateConfigDocument(doc); expect(taskRef(doc, "talk")).toBeUndefined();
    expect(migrateConfigDocument(doc)).toBe(false);
  });
  it("migrates legacy defaults, tasks, effort, rooms and manual caches exactly once", () => {
    const config = legacy(), originalPresets = structuredClone(config.presets);
    expect(migrateConfigDocument(config)).toBe(true);
    expect(config.default_ref).toEqual({ provider_id: "http", model: "manual" });
    expect(config.talk).toEqual({ enabled: true, model_ref: { provider_id: "http", model: "manual" }, reasoning_effort: "high" });
    expect(config.memory).toMatchObject({ reasoning_effort: "medium", embedding_ref: { provider_id: "http", model: "manual" }, untouched: true });
    expect(readProviders(config)[0]?.models).toHaveLength(301);
    expect(readProviders(config)[1]?.enabled).toBe(false);
    expect(readProviders(config).find(p => p.base_url === "mist-network://team")).toMatchObject({ provide: true, shared: [{ provider_id: "http", model: "manual" }] });
    expect(config.presets).toEqual(originalPresets);
    expect(config.unrelated).toEqual({ keep: true });
    const after = JSON.stringify(config);
    expect(migrateConfigDocument(config)).toBe(false);
    expect(JSON.stringify(config)).toBe(after);
  });
  it("preserves new refs and task effort over legacy IDs", () => {
    const config = legacy();
    config.talk = { preset_id: "old", model_ref: { provider_id: "disabled", model: "private" }, reasoning_effort: "none" };
    migrateConfigDocument(config);
    expect(taskRef(config, "talk")).toEqual({ providerId: "disabled", model: "private" });
    expect(resolveTaskModel(config, "talk")?.model).toBe("manual");
    expect((config.talk as Record<string, unknown>).reasoning_effort).toBe("none");
  });
  it("does not repoint refs when a provider is deleted and preserves unedited data", () => {
    const config = legacy(); migrateConfigDocument(config);
    const shared = sharedConfig(config); deleteProvider(shared, "http"); writeShared(config, shared);
    expect(taskRef(config, "talk")).toEqual({ providerId: "http", model: "manual" });
    expect(resolveTaskModel(config, "talk")).toBeNull();
    expect(config.unrelated).toEqual({ keep: true });
  });
  it("does not remigrate a task cleared after migration", () => {
    const config = legacy(); migrateConfigDocument(config); setTaskRef(config, "talk");
    expect(migrateConfigDocument(config)).toBe(false); expect(taskRef(config, "talk")).toBeUndefined();
  });
  it("keeps unresolved legacy IDs unusable and preserves room metadata", () => {
    const config = legacy(); config.vision = { preset_id: "gone" }; migrateConfigDocument(config);
    expect(taskRef(config, "vision")?.providerId).toBe("legacy:gone");
    const count = readProviders(config).length;
    addRoom(config, "team", "Renamed", true); expect(readProviders(config)).toHaveLength(count);
    expect(readProviders(config)[0]?.untouched).toBe(42);
  });
  it("shared migration keeps family-wide legacy fields unchanged and is idempotent", () => {
    const shared = sharedConfig(legacy()), presets = structuredClone(shared.presets), network = structuredClone(shared.network);
    expect(migrateSharedLlmConfig(shared).changed).toBe(true);
    expect(shared.presets).toEqual(presets); expect(shared.network).toEqual(network);
    expect(migrateSharedLlmConfig(shared).changed).toBe(false);
    expect(resolveModel(shared)?.model).toBe("manual");
  });
});