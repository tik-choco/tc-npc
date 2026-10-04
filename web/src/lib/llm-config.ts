// REST owns config and credentials; mistai owns ModelRef semantics.
import { emptyLlmConfig, isModelRef, migrateSharedLlmConfig, presetIdToRef,
  resolveModel, providerKind, createRoomProvider,
  type ModelRefV1, type SharedLlmConfigV1 } from "@tik-choco/mistai/llm-config";
import type { ConfigDocument } from "./types";
import type { MessageKey } from "./i18n";
import type { ModelRef, ProviderEntry, PresetEntry } from "./config-types";

export type Mutate = (updater: (draft: ConfigDocument) => void) => void;
export type LlmTaskId = "talk" | "memory" | "embedding" | "vision" | "action" | "translation" | "tts" | "stt";
export const LLM_TASKS: { id: LlmTaskId; section: string; field: string; labelKey: MessageKey; tipKey: MessageKey }[] =
  ["talk", "memory", "embedding", "vision", "action", "translation", "tts", "stt"].map(id => ({
    id: id as LlmTaskId, section: id === "embedding" ? "memory" : id,
    field: id === "embedding" ? "embedding_ref" : "model_ref",
    labelKey: `task.${id}` as MessageKey, tipKey: `task.${id}.tip` as MessageKey,
  }));
export function readProviders(config: ConfigDocument | null): ProviderEntry[] {
  return Array.isArray(config?.providers) ? config.providers as ProviderEntry[] : [];
}
export function fromRef(value: unknown): ModelRefV1 | undefined {
  if (!value || typeof value !== "object") return undefined;
  const ref = value as ModelRef;
  const converted = { providerId: ref.provider_id, model: ref.model };
  return isModelRef(converted) ? converted : undefined;
}
export function toRef(ref?: ModelRefV1): ModelRef | undefined {
  return ref ? { provider_id: ref.providerId, model: ref.model } : undefined;
}
export function sharedConfig(config: ConfigDocument | null): SharedLlmConfigV1 {
  const value = emptyLlmConfig();
  value.providers = readProviders(config).map(p => ({
    id: p.id, label: p.label || p.id, baseUrl: p.base_url ?? "", apiKey: p.api_key ?? "",
    enabled: p.enabled, models: p.models, modelsFetchedAt: p.models_fetched_at,
  }));
  value.defaultModel = fromRef(config?.default_ref);
  const presets = Array.isArray(config?.presets) ? config.presets as PresetEntry[] : [];
  value.presets = presets.map(p => ({ id: p.id, label: p.label || p.id,
    providerId: p.provider_id ?? "", model: p.model ?? "", reasoningEffort: p.reasoning_effort }));
  value.defaultPresetId = typeof config?.default_preset_id === "string" ? config.default_preset_id : "";
  const mist = config?.mist as Record<string, unknown> | undefined;
  value.network.roomId = typeof mist?.room_id === "string" ? mist.room_id : "";
  return value;
}
export function writeShared(draft: ConfigDocument, value: SharedLlmConfigV1): void {
  const old = readProviders(draft);
  draft.providers = value.providers.map(p => ({ ...old.find(o => o.id === p.id),
    id: p.id, label: p.label, base_url: p.baseUrl, api_key: p.apiKey,
    enabled: p.enabled, models: p.models, models_fetched_at: p.modelsFetchedAt,
  }));
  draft.default_ref = toRef(value.defaultModel);
}
export function taskRef(config: ConfigDocument | null, task: LlmTaskId): ModelRefV1 | undefined {
  const def = LLM_TASKS.find(t => t.id === task);
  const section = config?.[def?.section ?? ""] as Record<string, unknown> | undefined;
  return fromRef(section?.[def?.field ?? ""]);
}
export function setTaskRef(draft: ConfigDocument, task: LlmTaskId, ref?: ModelRefV1): void {
  const def = LLM_TASKS.find(t => t.id === task);
  if (!def) return;
  const section = (draft[def.section] ?? {}) as Record<string, unknown>;
  draft[def.section] = { ...section, [def.field]: toRef(ref) };
}
export function resolveTaskModel(config: ConfigDocument | null, task: LlmTaskId) {
  return config ? resolveModel(sharedConfig(config), taskRef(config, task)) : null;
}
// Consuming legacy IDs once allows clearing an assignment without remigration.
// Unedited fields and historical preset data survive the REST round trip.
export function migrateConfigDocument(draft: ConfigDocument): boolean {
  const before = JSON.stringify(draft);
  const shared = sharedConfig(draft);
  const sharedChanged = migrateSharedLlmConfig(shared).changed;
  for (const task of LLM_TASKS) {
    const section = draft[task.section] as Record<string, unknown> | undefined;
    const legacyKey = task.id === "embedding" ? "embedding_preset_id" : "preset_id";
    const id = section?.[legacyKey];
    if (typeof id !== "string") continue;
    if (!fromRef(section?.[task.field]) && id) {
      const previous = shared.presets.find(p => p.id === id);
      const ref = presetIdToRef(shared, id) ?? (previous ? undefined : { providerId: `legacy:${id}`, model: id });
      setTaskRef(draft, task.id, ref);
      const next = draft[task.section] as Record<string, unknown>;
      const preset = shared.presets.find(p => p.id === id);
      if (task.id !== "embedding" && !next.reasoning_effort && preset?.reasoningEffort)
        next.reasoning_effort = preset.reasoningEffort;
    }
    delete (draft[task.section] as Record<string, unknown>)[legacyKey];
  }
  if (sharedChanged) writeShared(draft, shared);
  if (typeof draft.default_preset_id === "string") delete draft.default_preset_id;
  const mist = draft.mist as Record<string, unknown> | undefined;
  if (mist?.room_id) {
    const room = shared.providers.find(p => providerKind(p) === "room" && p.baseUrl === `mist-network://${mist.room_id}`);
    const provider = room && readProviders(draft).find(p => p.id === room.id);
    if (provider && provider.provide === undefined) {
      const ids = Array.isArray(draft.sharedPresetIds) ? draft.sharedPresetIds as string[] : [];
      provider.provide = draft.networkProviderEnabled === true;
      provider.shared = ids.map(id => presetIdToRef(shared, id)).filter(ref =>
        ref && shared.providers.some(p => p.id === ref.providerId && providerKind(p) === "http"))
        .map(ref => toRef(ref)!);
    }
  }
  if (mist?.room_id) delete mist.room_id;
  return before !== JSON.stringify(draft);
}
export function addRoom(draft: ConfigDocument, roomId: string, label: string, provide: boolean): string {
  const config = sharedConfig(draft);
  const { id } = createRoomProvider(config, { roomId, label });
  writeShared(draft, config);
  const room = readProviders(draft).find(p => p.id === id)!;
  room.enabled = true;
  room.provide = provide || room.provide === true;
  room.shared ??= [];
  return id;
}