// 設定 tab: structured forms over the server config document
// (GET/PUT /api/config), following the tik-choco suite's shared settings
// convention (tc-docs/drafts/llm-settings-common-v1.md): descriptions stay
// out of the layout and live in label `title` tooltips instead, and a model/
// voice list fetch doubles as the connection test (no separate "test"
// button). Persistence is silent autosave via useConfigDoc — there is no
// save button except on the raw-JSON tab, which is intentionally outside
// the autosave path (see its section below).
//
// Reflection timing differs by field: schedule/location/route edits (owned
// by the 予定/行動 tabs, not this view) apply live; everything edited here
// (API connection, module on/off, VRC/server settings) only takes effect
// after the app restarts. That's noted with a small badge per section
// rather than a paragraph, per the shared convention's "説明は最小限、詳細は
// ツールチップへ" principle. The one exception is the 一般 tab's display
// language, which is browser-local and switches instantly — it carries no
// restart badge for exactly that reason.
//
// Config sections are typed via lib/config-types.ts where a shared type
// exists (api/tts/stt/vrc/enabled-only sections); `server` has no shared
// type (only the schedule/action-map/settings views touch config, and none
// of the others need `server`), so a small local interface covers it here.
// Every write spreads the existing section object before applying a patch,
// so fields this view doesn't know about survive the round-trip.
import type { ComponentChildren } from "preact";
import { useEffect, useMemo, useState } from "preact/hooks";
import { Blocks, Braces, Cpu, Globe, Loader2, RefreshCw, RotateCcw, Save, Volume2 } from "lucide-preact";
import { getConfig, putConfig, listModels, listVoices } from "../lib/api";
import { useConfigDoc } from "../hooks/useConfigDoc";
import { useI18n } from "../hooks/useI18n";
import { LANGS, type Lang, type MessageKey, type Translate } from "../lib/i18n";
import { SaveChip } from "../components/SaveChip";
import type {
  ApiSection,
  EnabledSection,
  SpeechEndpointSection,
  VrcSection,
} from "../lib/config-types";
import type { ConfigDocument } from "../lib/types";
import "../styles/components.css";
import "../styles/settings.css";

/** config.server — not in lib/config-types.ts (no other view touches it). */
interface ServerSectionLocal {
  addr?: string;
  auto_open?: boolean;
  [key: string]: unknown;
}

function section<T>(config: ConfigDocument | null, key: string): T {
  return ((config?.[key] as T | undefined) ?? ({} as T));
}

function commitOnEnter(event: KeyboardEvent): void {
  if (event.key === "Enter") (event.currentTarget as HTMLElement).blur();
}

// --- Generic blur-commit text field ---------------------------------------
// onInput only updates local draft state; the edit is written back to the
// config document (via onCommit -> mutate) on blur, or on Enter (which just
// triggers blur). This matches the tc-town reference implementation and
// avoids a PUT per keystroke.

function TextField(props: {
  label: string;
  tooltip?: string;
  value: string;
  placeholder?: string;
  type?: "text" | "password";
  onCommit: (value: string) => void;
}) {
  const { label, tooltip, value, placeholder, type = "text", onCommit } = props;
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);

  return (
    <label class="field" title={tooltip}>
      <span>{label}</span>
      <input
        type={type}
        value={draft}
        placeholder={placeholder}
        autoComplete="off"
        onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
        onBlur={() => {
          if (draft !== value) onCommit(draft);
        }}
        onKeyDown={commitOnEnter}
      />
    </label>
  );
}

function ToggleField(props: { label: string; tooltip?: string; checked: boolean; onChange: (v: boolean) => void }) {
  const { label, tooltip, checked, onChange } = props;
  return (
    <label class="settings-toggle" title={tooltip}>
      <input type="checkbox" checked={checked} onChange={(e) => onChange((e.target as HTMLInputElement).checked)} />
      <span>{label}</span>
    </label>
  );
}

/** A closed set of values — unlike ModelPicker there is nothing to fetch and
 * no manual-entry escape hatch, so it's a plain <select> in a field row.
 * `badge` rides on the label because the 一般 tab mixes a browser-local
 * setting with a restart-required one in the same card — a section-level
 * badge would wrongly cover both. */
function SelectField(props: {
  label: string;
  tooltip?: string;
  hint?: string;
  badge?: ComponentChildren;
  value: string;
  options: Array<{ value: string; label: string }>;
  onChange: (value: string) => void;
}) {
  const { label, tooltip, hint, badge, value, options, onChange } = props;
  return (
    <label class="field" title={tooltip}>
      <span>
        {label}
        {badge ? <span class="settings-field-badge">{badge}</span> : null}
      </span>
      <select value={value} onChange={(e) => onChange((e.target as HTMLSelectElement).value)}>
        {options.map((opt) => (
          <option key={opt.value} value={opt.value}>
            {opt.label}
          </option>
        ))}
      </select>
      {hint ? <span class="field-hint">{hint}</span> : null}
    </label>
  );
}

/** 推論エフォート — a 5-step ordinal scale, so it's a segmented control rather
 * than a <select>: all steps stay visible and the hint line explains the one
 * that's picked. Not a <label> wrapper (a radiogroup isn't a single control);
 * the radios carry the semantics and give arrow-key navigation for free. */
function ReasoningEffortField(props: { t: Translate; value: string; onChange: (value: string) => void }) {
  const { t, value, onChange } = props;
  const active = REASONING_EFFORT_OPTIONS.find((o) => o.value === value);

  return (
    <div class="field">
      <span title={t("settings.effort.tooltip")}>{t("settings.effort.label")}</span>
      <div class="segmented">
        {REASONING_EFFORT_OPTIONS.map((opt) => (
          <label
            key={opt.value}
            class={`segmented-option${opt.value === value ? " is-active" : ""}`}
            title={t(opt.hintKey)}
          >
            <input
              type="radio"
              name="api-reasoning-effort"
              value={opt.value}
              checked={opt.value === value}
              onChange={() => onChange(opt.value)}
            />
            {opt.value}
          </label>
        ))}
      </div>
      <span class="field-hint">
        {active ? t(active.hintKey) : t("settings.effort.saved", { value })}
        <br />
        {t("settings.effort.note")}
      </span>
    </div>
  );
}

/** Small "反映は再起動後" marker — a tooltip-only badge, not a hint paragraph. */
function RestartBadge({ t }: { t: Translate }) {
  return (
    <span class="badge settings-restart-badge" title={t("settings.restart.tooltip")}>
      {t("settings.restart")}
    </span>
  );
}

// --- Model / voice picker --------------------------------------------------
// A <select> populated by POST /api/llm/models or /api/llm/voices (only on
// explicit refresh — the fetch itself is the connection test, per the
// shared settings convention), plus a manual-entry fallback for when the
// endpoint can't list values yet (including a 404 while the server side of
// this feature is still being built — errors never crash the field).

type PickerKind = "models" | "voices";
type PickerStatus = "idle" | "loading" | "error" | "done";

function ModelPicker(props: {
  t: Translate;
  value: string;
  placeholder: string;
  baseUrl: string;
  apiKey: string;
  section: "api" | "tts" | "stt";
  kind: PickerKind;
  itemLabel: string;
  onChange: (value: string) => void;
}) {
  const { t, value, placeholder, baseUrl, apiKey, section: sectionName, kind, itemLabel, onChange } = props;
  const [options, setOptions] = useState<string[]>([]);
  const [status, setStatus] = useState<PickerStatus>("idle");
  const [errorMessage, setErrorMessage] = useState("");
  const [manualEntry, setManualEntry] = useState(false);
  const [draft, setDraft] = useState(value);
  useEffect(() => setDraft(value), [value]);

  const canFetch = baseUrl.trim().length > 0;

  async function refresh() {
    if (!canFetch) return;
    setStatus("loading");
    setErrorMessage("");
    try {
      const req = { baseUrl, apiKey, section: sectionName };
      const list =
        kind === "models" ? (await listModels(req)).models : (await listVoices(req)).voices;
      setOptions(list);
      if (list.length === 0) {
        setStatus("error");
        setErrorMessage(t("picker.empty", { item: itemLabel }));
      } else {
        setStatus("done");
      }
    } catch (err) {
      setOptions([]);
      setStatus("error");
      setErrorMessage(err instanceof Error ? err.message : String(err));
    }
  }

  const selectableOptions = useMemo(() => {
    const merged = value.trim() ? [value, ...options] : options;
    return [...new Set(merged)].sort((a, b) => a.localeCompare(b));
  }, [options, value]);

  if (manualEntry) {
    return (
      <div class="settings-picker">
        <div class="settings-picker-row">
          <input
            value={draft}
            placeholder={placeholder}
            autoComplete="off"
            onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
            onBlur={() => {
              if (draft !== value) onChange(draft);
            }}
            onKeyDown={commitOnEnter}
          />
          <button type="button" class="btn btn-ghost btn-small" onClick={() => setManualEntry(false)}>
            {t("picker.fromList")}
          </button>
        </div>
      </div>
    );
  }

  return (
    <div class="settings-picker">
      <div class="settings-picker-row">
        <select value={value} onChange={(e) => onChange((e.target as HTMLSelectElement).value)}>
          {value.trim() === "" ? <option value="">{t("picker.unselected")}</option> : null}
          {selectableOptions.map((item) => (
            <option key={item} value={item}>
              {item}
            </option>
          ))}
        </select>
        <button
          type="button"
          class="icon-btn"
          onClick={refresh}
          disabled={status === "loading" || !canFetch}
          title={t("picker.refresh", { item: itemLabel })}
          aria-label={t("picker.refreshAria", { item: itemLabel })}
        >
          <RefreshCw size={14} class={status === "loading" ? "spin" : ""} />
        </button>
        <button type="button" class="btn btn-ghost btn-small" onClick={() => setManualEntry(true)}>
          {t("picker.manual")}
        </button>
      </div>
      {status === "error" ? <p class="settings-picker-message settings-picker-message-error">{errorMessage}</p> : null}
      {status === "done" ? (
        <p class="settings-picker-message">{t("picker.count", { count: options.length })}</p>
      ) : null}
    </div>
  );
}

// --- Raw JSON editor (詳細 tab) --------------------------------------------
// Kept from the previous implementation for edge cases the structured form
// doesn't cover. Deliberately outside the autosave path: explicit save /
// reload buttons, PUT sent verbatim (masked "***" api_key fields round-trip
// untouched as long as they aren't hand-edited).

function JsonEditor({ t }: { t: Translate }) {
  const [text, setText] = useState("");
  const [loading, setLoading] = useState(true);
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [notice, setNotice] = useState<string | null>(null);

  function load() {
    setLoading(true);
    setError(null);
    setNotice(null);
    getConfig()
      .then((config) => setText(JSON.stringify(config, null, 2)))
      .catch((err) => setError(err instanceof Error ? err.message : String(err)))
      .finally(() => setLoading(false));
  }

  useEffect(() => {
    load();
  }, []);

  async function save() {
    let parsed: Record<string, unknown>;
    try {
      parsed = JSON.parse(text);
    } catch (err) {
      setError(t("settings.json.invalid", { error: err instanceof Error ? err.message : String(err) }));
      return;
    }
    setSaving(true);
    setError(null);
    setNotice(null);
    try {
      await putConfig(parsed);
      setNotice(t("settings.json.saved"));
      load();
    } catch (err) {
      setError(err instanceof Error ? err.message : String(err));
    } finally {
      setSaving(false);
    }
  }

  return (
    <div class="settings-json">
      <div class="settings-toolbar">
        <p class="settings-json-note">{t("settings.json.note")}</p>
        <div class="settings-toolbar-actions">
          <button type="button" class="btn btn-ghost" onClick={load} disabled={loading || saving}>
            <RotateCcw size={14} />
            {t("settings.json.reload")}
          </button>
          <button type="button" class="btn btn-primary" onClick={save} disabled={loading || saving}>
            {saving ? <Loader2 size={14} class="spin" /> : <Save size={14} />}
            {t("settings.json.save")}
          </button>
        </div>
      </div>

      {error && <div class="settings-error">{error}</div>}
      {notice && <div class="settings-notice">{notice}</div>}

      {loading ? (
        <div class="empty-state">{t("common.loading")}</div>
      ) : (
        <textarea
          class="settings-editor"
          spellcheck={false}
          value={text}
          onInput={(e) => setText((e.target as HTMLTextAreaElement).value)}
        />
      )}
    </div>
  );
}

// --- Main view ---------------------------------------------------------

type SettingsTabId = "general" | "connection" | "voice" | "modules" | "json";

const SETTINGS_TABS: Array<{ id: SettingsTabId; labelKey: MessageKey; icon: typeof Cpu }> = [
  { id: "general", labelKey: "settings.tab.general", icon: Globe },
  { id: "connection", labelKey: "settings.tab.connection", icon: Cpu },
  { id: "voice", labelKey: "settings.tab.voice", icon: Volume2 },
  { id: "modules", labelKey: "settings.tab.modules", icon: Blocks },
  { id: "json", labelKey: "settings.tab.json", icon: Braces },
];

/** Shared llm-settings union (tc-docs/drafts/llm-settings-common-v1.md §2.3).
 * "none" is an explicit value that is always sent, not "don't send". */
const REASONING_EFFORT_OPTIONS: Array<{ value: string; hintKey: MessageKey }> = [
  { value: "none", hintKey: "settings.effort.none" },
  { value: "minimal", hintKey: "settings.effort.minimal" },
  { value: "low", hintKey: "settings.effort.low" },
  { value: "medium", hintKey: "settings.effort.medium" },
  { value: "high", hintKey: "settings.effort.high" },
];

/** `config.language` — what the NPC answers in, mirroring the Rust side's
 * `npc_core::config::language_instruction`. Distinct from the display
 * language above it, which never leaves the browser. */
const NPC_LANGUAGE_VALUES = ["auto", ...LANGS.map((l) => l.id)] as const;

export function SettingsView() {
  const { t, lang, setLang } = useI18n();
  const [activeTab, setActiveTab] = useState<SettingsTabId>("general");
  const { config, loadError, saveState, saveError, mutate } = useConfigDoc();

  function patchSection<T extends object>(key: string, patch: Partial<T>) {
    mutate((draft) => {
      const current = (draft[key] as T | undefined) ?? ({} as T);
      draft[key] = { ...current, ...patch };
    });
  }

  const api = section<ApiSection>(config, "api");
  const tts = section<SpeechEndpointSection>(config, "tts");
  const stt = section<SpeechEndpointSection>(config, "stt");
  const talk = section<EnabledSection>(config, "talk");
  const memory = section<EnabledSection>(config, "memory");
  const vision = section<EnabledSection>(config, "vision");
  const action = section<EnabledSection>(config, "action");
  const scheduler = section<EnabledSection>(config, "scheduler");
  const vrc = section<VrcSection>(config, "vrc");
  const server = section<ServerSectionLocal>(config, "server");

  // `language` is a top-level scalar rather than a section, so it can't go
  // through patchSection. Anything unrecognized (including the empty string
  // a default-constructed config leaves behind) reads back as "auto", which
  // is exactly how the server treats it.
  const rawLanguage = typeof config?.language === "string" ? config.language : "";
  const npcLanguage = (NPC_LANGUAGE_VALUES as readonly string[]).includes(rawLanguage)
    ? rawLanguage
    : "auto";

  const npcLanguageOptions = [
    { value: "auto", label: t("settings.general.npcLanguage.auto") },
    ...LANGS.map((l) => ({ value: l.id, label: l.label })),
  ];

  return (
    <div class="settings-view">
      <div class="settings-header">
        <h2 class="settings-title">{t("settings.title")}</h2>
        <SaveChip state={saveState} error={saveError} />
      </div>

      <div class="settings-tabs" role="tablist" aria-label={t("settings.tabsLabel")}>
        {SETTINGS_TABS.map((tab) => {
          const Icon = tab.icon;
          const selected = activeTab === tab.id;
          return (
            <button
              key={tab.id}
              type="button"
              role="tab"
              aria-selected={selected}
              class={`settings-tab${selected ? " settings-tab-active" : ""}`}
              onClick={() => setActiveTab(tab.id)}
            >
              <Icon size={15} />
              {t(tab.labelKey)}
            </button>
          );
        })}
      </div>

      {loadError ? (
        <div class="settings-error">
          {t("settings.loadError")}: {loadError}
        </div>
      ) : !config && activeTab !== "json" ? (
        <div class="empty-state">{t("common.loading")}</div>
      ) : (
        <>
          {activeTab === "general" && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>{t("settings.general.title")}</h3>
              </div>

              <div class="settings-card">
                <SelectField
                  label={t("settings.general.uiLanguage")}
                  tooltip={t("settings.general.uiLanguage.tooltip")}
                  hint={t("settings.general.uiLanguage.hint")}
                  value={lang}
                  options={LANGS.map((l) => ({ value: l.id, label: l.label }))}
                  onChange={(v) => setLang(v as Lang)}
                />
                <SelectField
                  label={t("settings.general.npcLanguage")}
                  tooltip={t("settings.general.npcLanguage.tooltip")}
                  hint={t("settings.general.npcLanguage.hint")}
                  badge={<RestartBadge t={t} />}
                  value={npcLanguage}
                  options={npcLanguageOptions}
                  onChange={(v) =>
                    mutate((draft) => {
                      draft.language = v;
                    })
                  }
                />
              </div>
            </section>
          )}

          {activeTab === "connection" && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>{t("settings.connection.title")}</h3>
                <RestartBadge t={t} />
              </div>
              <div class="settings-card">
                <TextField
                  label={t("settings.field.baseUrl")}
                  tooltip={t("settings.field.baseUrl.tooltip")}
                  value={api.base_url ?? ""}
                  placeholder="http://localhost:11434/v1"
                  onCommit={(v) => patchSection<ApiSection>("api", { base_url: v })}
                />
                <TextField
                  label={t("settings.field.apiKey")}
                  tooltip={t("settings.field.apiKey.tooltip")}
                  type="password"
                  value={api.api_key ?? ""}
                  placeholder="sk-..."
                  onCommit={(v) => patchSection<ApiSection>("api", { api_key: v })}
                />
                <label class="field">
                  <span>{t("settings.field.model")}</span>
                  <ModelPicker
                    t={t}
                    value={api.model ?? ""}
                    placeholder="gpt-4o-mini"
                    baseUrl={api.base_url ?? ""}
                    apiKey={api.api_key ?? ""}
                    section="api"
                    kind="models"
                    itemLabel={t("picker.item.model")}
                    onChange={(v) => patchSection<ApiSection>("api", { model: v })}
                  />
                </label>
                <label class="field" title={t("settings.field.embeddingModel.tooltip")}>
                  <span>{t("settings.field.embeddingModel")}</span>
                  <ModelPicker
                    t={t}
                    value={api.embedding_model ?? ""}
                    placeholder="text-embedding-3-small"
                    baseUrl={api.base_url ?? ""}
                    apiKey={api.api_key ?? ""}
                    section="api"
                    kind="models"
                    itemLabel={t("picker.item.model")}
                    onChange={(v) => patchSection<ApiSection>("api", { embedding_model: v })}
                  />
                </label>
                <ReasoningEffortField
                  t={t}
                  value={api.reasoning_effort || "none"}
                  onChange={(v) => patchSection<ApiSection>("api", { reasoning_effort: v })}
                />
              </div>
            </section>
          )}

          {activeTab === "voice" && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>{t("settings.voice.title")}</h3>
                <RestartBadge t={t} />
              </div>

              <div class="settings-card">
                <div class="settings-card-head">
                  <h4>{t("settings.tts.title")}</h4>
                  <ToggleField
                    label={t("common.enabled")}
                    checked={tts.enabled ?? false}
                    onChange={(v) => patchSection<SpeechEndpointSection>("tts", { enabled: v })}
                  />
                </div>
                <TextField
                  label={t("settings.field.baseUrl")}
                  tooltip={t("settings.fallback.baseUrl.tooltip")}
                  value={tts.base_url ?? ""}
                  placeholder={t("settings.fallback.placeholder")}
                  onCommit={(v) => patchSection<SpeechEndpointSection>("tts", { base_url: v })}
                />
                <TextField
                  label={t("settings.field.apiKey")}
                  tooltip={t("settings.fallback.apiKey.tooltip")}
                  type="password"
                  value={tts.api_key ?? ""}
                  placeholder={t("settings.fallback.placeholder")}
                  onCommit={(v) => patchSection<SpeechEndpointSection>("tts", { api_key: v })}
                />
                <label class="field">
                  <span>{t("settings.field.model")}</span>
                  <ModelPicker
                    t={t}
                    value={tts.model ?? ""}
                    placeholder="tts-1"
                    baseUrl={tts.base_url || api.base_url || ""}
                    apiKey={tts.api_key || api.api_key || ""}
                    section="tts"
                    kind="models"
                    itemLabel={t("picker.item.model")}
                    onChange={(v) => patchSection<SpeechEndpointSection>("tts", { model: v })}
                  />
                </label>
                <label class="field">
                  <span>{t("settings.field.ttsVoice")}</span>
                  <ModelPicker
                    t={t}
                    value={tts.voice ?? ""}
                    placeholder="alloy"
                    baseUrl={tts.base_url || api.base_url || ""}
                    apiKey={tts.api_key || api.api_key || ""}
                    section="tts"
                    kind="voices"
                    itemLabel={t("picker.item.voice")}
                    onChange={(v) => patchSection<SpeechEndpointSection>("tts", { voice: v })}
                  />
                </label>
              </div>

              <div class="settings-card">
                <div class="settings-card-head">
                  <h4>{t("settings.stt.title")}</h4>
                  <ToggleField
                    label={t("common.enabled")}
                    checked={stt.enabled ?? false}
                    onChange={(v) => patchSection<SpeechEndpointSection>("stt", { enabled: v })}
                  />
                </div>
                <TextField
                  label={t("settings.field.baseUrl")}
                  tooltip={t("settings.fallback.baseUrl.tooltip")}
                  value={stt.base_url ?? ""}
                  placeholder={t("settings.fallback.placeholder")}
                  onCommit={(v) => patchSection<SpeechEndpointSection>("stt", { base_url: v })}
                />
                <TextField
                  label={t("settings.field.apiKey")}
                  tooltip={t("settings.fallback.apiKey.tooltip")}
                  type="password"
                  value={stt.api_key ?? ""}
                  placeholder={t("settings.fallback.placeholder")}
                  onCommit={(v) => patchSection<SpeechEndpointSection>("stt", { api_key: v })}
                />
                <label class="field">
                  <span>{t("settings.field.model")}</span>
                  <ModelPicker
                    t={t}
                    value={stt.model ?? ""}
                    placeholder="whisper-1"
                    baseUrl={stt.base_url || api.base_url || ""}
                    apiKey={stt.api_key || api.api_key || ""}
                    section="stt"
                    kind="models"
                    itemLabel={t("picker.item.model")}
                    onChange={(v) => patchSection<SpeechEndpointSection>("stt", { model: v })}
                  />
                </label>
              </div>
            </section>
          )}

          {activeTab === "modules" && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>{t("settings.modules.title")}</h3>
                <RestartBadge t={t} />
              </div>

              <div class="settings-card">
                <h4>{t("settings.modules.card")}</h4>
                <div class="settings-toggle-grid">
                  <ToggleField
                    label={t("settings.module.talk")}
                    checked={talk.enabled ?? false}
                    onChange={(v) => patchSection<EnabledSection>("talk", { enabled: v })}
                  />
                  <ToggleField
                    label={t("settings.module.memory")}
                    checked={memory.enabled ?? false}
                    onChange={(v) => patchSection<EnabledSection>("memory", { enabled: v })}
                  />
                  <ToggleField
                    label={t("settings.module.vision")}
                    checked={vision.enabled ?? false}
                    onChange={(v) => patchSection<EnabledSection>("vision", { enabled: v })}
                  />
                  <ToggleField
                    label={t("settings.module.action")}
                    checked={action.enabled ?? false}
                    onChange={(v) => patchSection<EnabledSection>("action", { enabled: v })}
                  />
                  <ToggleField
                    label={t("settings.module.scheduler")}
                    checked={scheduler.enabled ?? false}
                    onChange={(v) => patchSection<EnabledSection>("scheduler", { enabled: v })}
                  />
                </div>
              </div>

              <div class="settings-card">
                <h4>{t("settings.vrc.title")}</h4>
                <ToggleField
                  label={t("settings.vrc.chatbox")}
                  checked={vrc.chatbox ?? false}
                  onChange={(v) => patchSection<VrcSection>("vrc", { chatbox: v })}
                />
                <TextField
                  label={t("settings.vrc.osc")}
                  tooltip={t("settings.vrc.osc.tooltip")}
                  value={vrc.osc_address ?? ""}
                  placeholder="127.0.0.1:9000"
                  onCommit={(v) => patchSection<VrcSection>("vrc", { osc_address: v })}
                />
              </div>

              <div class="settings-card">
                <h4>{t("settings.server.title")}</h4>
                <TextField
                  label={t("settings.server.addr")}
                  tooltip={t("settings.server.addr.tooltip")}
                  value={server.addr ?? ""}
                  placeholder="127.0.0.1:47950"
                  onCommit={(v) => patchSection<ServerSectionLocal>("server", { addr: v })}
                />
                <ToggleField
                  label={t("settings.server.autoOpen")}
                  checked={server.auto_open ?? false}
                  onChange={(v) => patchSection<ServerSectionLocal>("server", { auto_open: v })}
                />
              </div>
            </section>
          )}

          {activeTab === "json" && (
            <section class="settings-section">
              <JsonEditor t={t} />
            </section>
          )}
        </>
      )}
    </div>
  );
}
