// Provider/preset config model shared by the AI settings UI (SettingsView.tsx
// and friends). Mirrors the shape the Rust side owns (see
// tc-docs/drafts/llm-settings-common-v1.md §2 for the design this ports):
// `providers[]` is "where to connect", `presets[]` is "how to call it" (a
// model + reasoning effort bound to a provider), and each task section
// (talk/memory/vision/action/translation/tts/stt, plus memory's embedding
// slot) stores only a `preset_id` — empty means "follow default_preset_id".
//
// This module is the single source of truth for reading/resolving/mutating
// that shape so the settings view and any card-list UI agree on semantics.
// All mutation helpers go through the `Mutate` callback from useConfigDoc and
// always spread the existing section/array before patching, so fields this
// module doesn't know about survive the round-trip.
import type { ConfigDocument } from "./types";
import type { MessageKey } from "./i18n";
import type { ProviderEntry, PresetEntry } from "./config-types";

export type Mutate = (updater: (draft: ConfigDocument) => void) => void;

export type LlmTaskId =
  | "talk"
  | "memory"
  | "embedding"
  | "vision"
  | "action"
  | "translation"
  | "tts"
  | "stt";

/** One row of the タスク tab: which config section/field it reads its
 * preset assignment from, and the i18n keys for its label/tooltip. */
export interface LlmTaskDef {
  id: LlmTaskId;
  /** config section key. `embedding` also points at "memory" (a second
   * field on the same section, not a section of its own). */
  section: string;
  /** Field name within that section. */
  field: "preset_id" | "embedding_preset_id";
  labelKey: MessageKey;
  tipKey: MessageKey;
}

/** Single source of truth for the タスク tab's row order. */
export const LLM_TASKS: LlmTaskDef[] = [
  { id: "talk", section: "talk", field: "preset_id", labelKey: "task.talk", tipKey: "task.talk.tip" },
  { id: "memory", section: "memory", field: "preset_id", labelKey: "task.memory", tipKey: "task.memory.tip" },
  {
    id: "embedding",
    section: "memory",
    field: "embedding_preset_id",
    labelKey: "task.embedding",
    tipKey: "task.embedding.tip",
  },
  { id: "vision", section: "vision", field: "preset_id", labelKey: "task.vision", tipKey: "task.vision.tip" },
  { id: "action", section: "action", field: "preset_id", labelKey: "task.action", tipKey: "task.action.tip" },
  {
    id: "translation",
    section: "translation",
    field: "preset_id",
    labelKey: "task.translation",
    tipKey: "task.translation.tip",
  },
  { id: "tts", section: "tts", field: "preset_id", labelKey: "task.tts", tipKey: "task.tts.tip" },
  { id: "stt", section: "stt", field: "preset_id", labelKey: "task.stt", tipKey: "task.stt.tip" },
];

function taskDef(task: LlmTaskId): LlmTaskDef | undefined {
  return LLM_TASKS.find((t) => t.id === task);
}

function section<T>(config: ConfigDocument | null, key: string): T {
  return (config?.[key] as T | undefined) ?? ({} as T);
}

// --- Reads -------------------------------------------------------------

export function readProviders(config: ConfigDocument | null): ProviderEntry[] {
  const list = config?.providers;
  return Array.isArray(list) ? (list as ProviderEntry[]) : [];
}

export function readPresets(config: ConfigDocument | null): PresetEntry[] {
  const list = config?.presets;
  return Array.isArray(list) ? (list as PresetEntry[]) : [];
}

export function readDefaultPresetId(config: ConfigDocument | null): string {
  const id = config?.default_preset_id;
  return typeof id === "string" ? id : "";
}

/** Look up a preset by id. `presetId === ""` means "follow the default
 * preset" — this resolves default_preset_id instead. Returns null if the
 * (possibly resolved) id doesn't match any preset, including a dangling
 * default_preset_id. */
export function resolvePreset(config: ConfigDocument | null, presetId: string): PresetEntry | null {
  const id = presetId || readDefaultPresetId(config);
  if (!id) return null;
  return readPresets(config).find((p) => p.id === id) ?? null;
}

export function providerOf(config: ConfigDocument | null, preset: PresetEntry | null): ProviderEntry | null {
  if (!preset?.provider_id) return null;
  return readProviders(config).find((p) => p.id === preset.provider_id) ?? null;
}

/** The raw assignment stored on a task's section. `""` means "follow the
 * default preset" and is returned as-is (not resolved) — callers that need
 * the effective preset should use `resolveTaskPreset`. */
export function taskPresetId(config: ConfigDocument | null, task: LlmTaskId): string {
  const def = taskDef(task);
  if (!def) return "";
  const sec = section<Record<string, unknown>>(config, def.section);
  const value = sec[def.field];
  return typeof value === "string" ? value : "";
}

/** The preset a task actually uses right now: its own assignment, or the
 * default preset if it has none. */
export function resolveTaskPreset(config: ConfigDocument | null, task: LlmTaskId): PresetEntry | null {
  return resolvePreset(config, taskPresetId(config, task));
}

/** Display label for a preset card: its own label, falling back to the
 * model name, falling back to the id (a preset always has one of the
 * three non-empty). */
export function presetLabel(preset: PresetEntry): string {
  return preset.label?.trim() || preset.model?.trim() || preset.id;
}

/** Display label for a provider card: its own label, falling back to the id. */
export function providerLabel(provider: ProviderEntry): string {
  return provider.label?.trim() || provider.id;
}

/** Smallest `${prefix}N` (N starting at 1) not already present in
 * `existing` — deterministic (no Date.now/Math.random) so tests and
 * optimistic UI updates stay predictable. */
export function newId(prefix: string, existing: string[]): string {
  const used = new Set(existing);
  let n = 1;
  while (used.has(`${prefix}${n}`)) n++;
  return `${prefix}${n}`;
}

/** Badges to show on a preset card: "既定" if it IS the default preset
 * (exact id match against default_preset_id — a task following the default
 * via "" does not itself make this preset "the default"), plus one badge
 * per task whose *effective* preset (see resolveTaskPreset) is this one —
 * which also covers tasks that get here by following the default. */
export function presetBadgeKeys(config: ConfigDocument | null, presetId: string): MessageKey[] {
  const keys: MessageKey[] = [];
  if (presetId && presetId === readDefaultPresetId(config)) {
    keys.push("llm.badge.default");
  }
  for (const task of LLM_TASKS) {
    if (resolveTaskPreset(config, task.id)?.id === presetId) keys.push(task.labelKey);
  }
  return keys;
}

/** How many presets currently point at this provider — used to warn before
 * deleting a provider that's still in use. */
export function presetCountForProvider(config: ConfigDocument | null, providerId: string): number {
  return readPresets(config).filter((p) => p.provider_id === providerId).length;
}

// --- Writes --------------------------------------------------------------
// All mutation goes through `mutate` (from useConfigDoc), which hands us a
// deep-cloned draft to edit in place; the caller doesn't see the result
// directly, so every helper below is fire-and-forget from its perspective.

/** Clears default_preset_id and any task's preset assignment that points at
 * one of `removedIds`, so deleting a preset/provider never leaves a
 * dangling reference behind (it falls back to "" = follow default). */
function pruneDanglingPresetRefs(draft: ConfigDocument, removedIds: Set<string>): void {
  if (typeof draft.default_preset_id === "string" && removedIds.has(draft.default_preset_id)) {
    draft.default_preset_id = "";
  }
  for (const task of LLM_TASKS) {
    const current = draft[task.section] as Record<string, unknown> | undefined;
    if (!current) continue;
    const value = current[task.field];
    if (typeof value === "string" && removedIds.has(value)) {
      draft[task.section] = { ...current, [task.field]: "" };
    }
  }
}

export function addProvider(mutate: Mutate, entry: ProviderEntry): void {
  mutate((draft) => {
    const providers = Array.isArray(draft.providers) ? (draft.providers as ProviderEntry[]) : [];
    draft.providers = [...providers, entry];
  });
}

export function updateProvider(mutate: Mutate, id: string, patch: Partial<ProviderEntry>): void {
  mutate((draft) => {
    const providers = Array.isArray(draft.providers) ? (draft.providers as ProviderEntry[]) : [];
    draft.providers = providers.map((p) => (p.id === id ? { ...p, ...patch } : p));
  });
}

/** Deletes a provider along with any preset that references it, then clears
 * default_preset_id / task assignments left dangling by those preset
 * deletions (a provider's presets can be assigned to tasks too). */
export function deleteProvider(mutate: Mutate, id: string): void {
  mutate((draft) => {
    const providers = Array.isArray(draft.providers) ? (draft.providers as ProviderEntry[]) : [];
    draft.providers = providers.filter((p) => p.id !== id);

    const presets = Array.isArray(draft.presets) ? (draft.presets as PresetEntry[]) : [];
    const removedIds = new Set(presets.filter((p) => p.provider_id === id).map((p) => p.id));
    draft.presets = presets.filter((p) => p.provider_id !== id);

    pruneDanglingPresetRefs(draft, removedIds);
  });
}

export function addPreset(mutate: Mutate, entry: PresetEntry): void {
  mutate((draft) => {
    const presets = Array.isArray(draft.presets) ? (draft.presets as PresetEntry[]) : [];
    draft.presets = [...presets, entry];
  });
}

export function updatePreset(mutate: Mutate, id: string, patch: Partial<PresetEntry>): void {
  mutate((draft) => {
    const presets = Array.isArray(draft.presets) ? (draft.presets as PresetEntry[]) : [];
    draft.presets = presets.map((p) => (p.id === id ? { ...p, ...patch } : p));
  });
}

/** Deletes a preset, then clears default_preset_id / any task's assignment
 * that pointed at it (both fall back to "" = follow default). */
export function deletePreset(mutate: Mutate, id: string): void {
  mutate((draft) => {
    const presets = Array.isArray(draft.presets) ? (draft.presets as PresetEntry[]) : [];
    draft.presets = presets.filter((p) => p.id !== id);
    pruneDanglingPresetRefs(draft, new Set([id]));
  });
}

export function setDefaultPreset(mutate: Mutate, presetId: string): void {
  mutate((draft) => {
    draft.default_preset_id = presetId;
  });
}

export function setTaskPreset(mutate: Mutate, task: LlmTaskId, presetId: string): void {
  const def = taskDef(task);
  if (!def) return;
  mutate((draft) => {
    const current = (draft[def.section] as Record<string, unknown> | undefined) ?? {};
    draft[def.section] = { ...current, [def.field]: presetId };
  });
}
