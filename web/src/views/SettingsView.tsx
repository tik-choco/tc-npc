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
// ツールチップへ" principle.
//
// Config sections are typed via lib/config-types.ts where a shared type
// exists (api/tts/stt/vrc/enabled-only sections); `server` has no shared
// type (only the schedule/action-map/settings views touch config, and none
// of the others need `server`), so a small local interface covers it here.
// Every write spreads the existing section object before applying a patch,
// so fields this view doesn't know about survive the round-trip.
import { useEffect, useMemo, useState } from "preact/hooks";
import { Blocks, Braces, Cpu, Loader2, RefreshCw, RotateCcw, Save, Volume2 } from "lucide-preact";
import { getConfig, putConfig, listModels, listVoices } from "../lib/api";
import { useConfigDoc } from "../hooks/useConfigDoc";
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

/** Small "反映は再起動後" marker — a tooltip-only badge, not a hint paragraph. */
function RestartBadge() {
  return (
    <span class="badge settings-restart-badge" title="この設定はアプリの再起動後に反映されます">
      再起動後に反映
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
  value: string;
  placeholder: string;
  baseUrl: string;
  apiKey: string;
  section: "api" | "tts" | "stt";
  kind: PickerKind;
  itemLabel: string;
  onChange: (value: string) => void;
}) {
  const { value, placeholder, baseUrl, apiKey, section: sectionName, kind, itemLabel, onChange } = props;
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
        setErrorMessage(`${itemLabel}が0件でした。接続設定を確認してください。`);
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
            一覧から選択
          </button>
        </div>
      </div>
    );
  }

  return (
    <div class="settings-picker">
      <div class="settings-picker-row">
        <select value={value} onChange={(e) => onChange((e.target as HTMLSelectElement).value)}>
          {value.trim() === "" ? <option value="">未選択</option> : null}
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
          title={`${itemLabel}一覧を取得（接続テストを兼ねます）`}
          aria-label={`${itemLabel}一覧を更新`}
        >
          <RefreshCw size={14} class={status === "loading" ? "spin" : ""} />
        </button>
        <button type="button" class="btn btn-ghost btn-small" onClick={() => setManualEntry(true)}>
          手入力
        </button>
      </div>
      {status === "error" ? <p class="settings-picker-message settings-picker-message-error">{errorMessage}</p> : null}
      {status === "done" ? <p class="settings-picker-message">{options.length} 件取得しました</p> : null}
    </div>
  );
}

// --- Raw JSON editor (詳細 tab) --------------------------------------------
// Kept from the previous implementation for edge cases the structured form
// doesn't cover. Deliberately outside the autosave path: explicit save /
// reload buttons, PUT sent verbatim (masked "***" api_key fields round-trip
// untouched as long as they aren't hand-edited).

function JsonEditor() {
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
      setError(`JSON が不正です: ${err instanceof Error ? err.message : String(err)}`);
      return;
    }
    setSaving(true);
    setError(null);
    setNotice(null);
    try {
      await putConfig(parsed);
      setNotice("設定を保存しました");
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
        <p class="settings-json-note">
          このタブは自動保存の対象外です。保存ボタンで明示的に反映してください（"***" のままの API
          キーはサーバー側の保存値を維持します）。
        </p>
        <div class="settings-toolbar-actions">
          <button type="button" class="btn btn-ghost" onClick={load} disabled={loading || saving}>
            <RotateCcw size={14} />
            再読み込み
          </button>
          <button type="button" class="btn btn-primary" onClick={save} disabled={loading || saving}>
            {saving ? <Loader2 size={14} class="spin" /> : <Save size={14} />}
            保存
          </button>
        </div>
      </div>

      {error && <div class="settings-error">{error}</div>}
      {notice && <div class="settings-notice">{notice}</div>}

      {loading ? (
        <div class="empty-state">読み込み中…</div>
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

type SettingsTabId = "connection" | "voice" | "modules" | "json";

const SETTINGS_TABS: Array<{ id: SettingsTabId; label: string; icon: typeof Cpu }> = [
  { id: "connection", label: "AI接続", icon: Cpu },
  { id: "voice", label: "音声", icon: Volume2 },
  { id: "modules", label: "モジュール・連携", icon: Blocks },
  { id: "json", label: "詳細 (JSON)", icon: Braces },
];

const REASONING_EFFORT_OPTIONS = ["", "low", "medium", "high"];

export function SettingsView() {
  const [activeTab, setActiveTab] = useState<SettingsTabId>("connection");
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

  return (
    <div class="settings-view">
      <div class="settings-header">
        <h2 class="settings-title">設定</h2>
        {saveState !== "idle" ? (
          <span
            class={`chip settings-save-chip${
              saveState === "saved" ? " chip--open" : saveState === "saving" ? " chip--connecting" : " chip--closed"
            }`}
            title={saveState === "error" ? (saveError ?? "") : undefined}
          >
            <span class="chip-dot" />
            {saveState === "saving" ? "保存中…" : saveState === "saved" ? "保存しました" : "保存に失敗しました"}
          </span>
        ) : null}
      </div>

      <div class="settings-tabs" role="tablist" aria-label="設定タブ">
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
              {tab.label}
            </button>
          );
        })}
      </div>

      {loadError ? (
        <div class="settings-error">設定の読み込みに失敗しました: {loadError}</div>
      ) : !config && activeTab !== "json" ? (
        <div class="empty-state">読み込み中…</div>
      ) : (
        <>
          {activeTab === "connection" && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>AI接続</h3>
                <RestartBadge />
              </div>
              <div class="settings-card">
                <TextField
                  label="ベース URL"
                  tooltip="OpenAI 互換 API のエンドポイント（例: http://localhost:11434/v1）"
                  value={api.base_url ?? ""}
                  placeholder="http://localhost:11434/v1"
                  onCommit={(v) => patchSection<ApiSection>("api", { base_url: v })}
                />
                <TextField
                  label="API キー"
                  tooltip='"***" のままなら保存済みの値を維持します。書き換えると新しい値で上書きされます。'
                  type="password"
                  value={api.api_key ?? ""}
                  placeholder="sk-..."
                  onCommit={(v) => patchSection<ApiSection>("api", { api_key: v })}
                />
                <label class="field">
                  <span>モデル</span>
                  <ModelPicker
                    value={api.model ?? ""}
                    placeholder="gpt-4o-mini"
                    baseUrl={api.base_url ?? ""}
                    apiKey={api.api_key ?? ""}
                    section="api"
                    kind="models"
                    itemLabel="モデル"
                    onChange={(v) => patchSection<ApiSection>("api", { model: v })}
                  />
                </label>
                <TextField
                  label="埋め込みモデル"
                  tooltip="記憶（memory）の検索に使う embedding モデル名です。"
                  value={api.embedding_model ?? ""}
                  placeholder="text-embedding-3-small"
                  onCommit={(v) => patchSection<ApiSection>("api", { embedding_model: v })}
                />
                <label class="field" title="OpenAI 互換 API の reasoning_effort パラメータです。空欄は送信しません。">
                  <span>推論エフォート</span>
                  <select
                    value={api.reasoning_effort ?? ""}
                    onChange={(e) =>
                      patchSection<ApiSection>("api", { reasoning_effort: (e.target as HTMLSelectElement).value })
                    }
                  >
                    {REASONING_EFFORT_OPTIONS.map((opt) => (
                      <option key={opt} value={opt}>
                        {opt === "" ? "未設定" : opt}
                      </option>
                    ))}
                  </select>
                </label>
              </div>
            </section>
          )}

          {activeTab === "voice" && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>音声</h3>
                <RestartBadge />
              </div>

              <div class="settings-card">
                <div class="settings-card-head">
                  <h4>読み上げ (TTS)</h4>
                  <ToggleField
                    label="有効"
                    checked={tts.enabled ?? false}
                    onChange={(v) => patchSection<SpeechEndpointSection>("tts", { enabled: v })}
                  />
                </div>
                <TextField
                  label="ベース URL"
                  tooltip="空欄なら AI接続タブのベース URL を使います。"
                  value={tts.base_url ?? ""}
                  placeholder="（未設定 = AI接続の値を使用）"
                  onCommit={(v) => patchSection<SpeechEndpointSection>("tts", { base_url: v })}
                />
                <TextField
                  label="API キー"
                  tooltip='空欄なら AI接続タブの API キーを使います。"***" のままなら保存済みの値を維持します。'
                  type="password"
                  value={tts.api_key ?? ""}
                  placeholder="（未設定 = AI接続の値を使用）"
                  onCommit={(v) => patchSection<SpeechEndpointSection>("tts", { api_key: v })}
                />
                <label class="field">
                  <span>モデル</span>
                  <ModelPicker
                    value={tts.model ?? ""}
                    placeholder="tts-1"
                    baseUrl={tts.base_url || api.base_url || ""}
                    apiKey={tts.api_key || api.api_key || ""}
                    section="tts"
                    kind="models"
                    itemLabel="モデル"
                    onChange={(v) => patchSection<SpeechEndpointSection>("tts", { model: v })}
                  />
                </label>
                <label class="field">
                  <span>声</span>
                  <ModelPicker
                    value={tts.voice ?? ""}
                    placeholder="alloy"
                    baseUrl={tts.base_url || api.base_url || ""}
                    apiKey={tts.api_key || api.api_key || ""}
                    section="tts"
                    kind="voices"
                    itemLabel="音声"
                    onChange={(v) => patchSection<SpeechEndpointSection>("tts", { voice: v })}
                  />
                </label>
              </div>

              <div class="settings-card">
                <div class="settings-card-head">
                  <h4>書き起こし (STT)</h4>
                  <ToggleField
                    label="有効"
                    checked={stt.enabled ?? false}
                    onChange={(v) => patchSection<SpeechEndpointSection>("stt", { enabled: v })}
                  />
                </div>
                <TextField
                  label="ベース URL"
                  tooltip="空欄なら AI接続タブのベース URL を使います。"
                  value={stt.base_url ?? ""}
                  placeholder="（未設定 = AI接続の値を使用）"
                  onCommit={(v) => patchSection<SpeechEndpointSection>("stt", { base_url: v })}
                />
                <TextField
                  label="API キー"
                  tooltip='空欄なら AI接続タブの API キーを使います。"***" のままなら保存済みの値を維持します。'
                  type="password"
                  value={stt.api_key ?? ""}
                  placeholder="（未設定 = AI接続の値を使用）"
                  onCommit={(v) => patchSection<SpeechEndpointSection>("stt", { api_key: v })}
                />
                <label class="field">
                  <span>モデル</span>
                  <ModelPicker
                    value={stt.model ?? ""}
                    placeholder="whisper-1"
                    baseUrl={stt.base_url || api.base_url || ""}
                    apiKey={stt.api_key || api.api_key || ""}
                    section="stt"
                    kind="models"
                    itemLabel="モデル"
                    onChange={(v) => patchSection<SpeechEndpointSection>("stt", { model: v })}
                  />
                </label>
              </div>
            </section>
          )}

          {activeTab === "modules" && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>モジュール・連携</h3>
                <RestartBadge />
              </div>

              <div class="settings-card">
                <h4>機能モジュール</h4>
                <div class="settings-toggle-grid">
                  <ToggleField
                    label="会話 (talk)"
                    checked={talk.enabled ?? false}
                    onChange={(v) => patchSection<EnabledSection>("talk", { enabled: v })}
                  />
                  <ToggleField
                    label="記憶 (memory)"
                    checked={memory.enabled ?? false}
                    onChange={(v) => patchSection<EnabledSection>("memory", { enabled: v })}
                  />
                  <ToggleField
                    label="視覚 (vision)"
                    checked={vision.enabled ?? false}
                    onChange={(v) => patchSection<EnabledSection>("vision", { enabled: v })}
                  />
                  <ToggleField
                    label="行動 (action)"
                    checked={action.enabled ?? false}
                    onChange={(v) => patchSection<EnabledSection>("action", { enabled: v })}
                  />
                  <ToggleField
                    label="予定 (scheduler)"
                    checked={scheduler.enabled ?? false}
                    onChange={(v) => patchSection<EnabledSection>("scheduler", { enabled: v })}
                  />
                </div>
              </div>

              <div class="settings-card">
                <h4>VRChat 連携</h4>
                <ToggleField
                  label="Chatbox に発言を送る"
                  checked={vrc.chatbox ?? false}
                  onChange={(v) => patchSection<VrcSection>("vrc", { chatbox: v })}
                />
                <TextField
                  label="OSC アドレス"
                  tooltip="VRChat の OSC 送受信先（host:port）です。"
                  value={vrc.osc_address ?? ""}
                  placeholder="127.0.0.1:9000"
                  onCommit={(v) => patchSection<VrcSection>("vrc", { osc_address: v })}
                />
              </div>

              <div class="settings-card">
                <h4>サーバー</h4>
                <TextField
                  label="待受アドレス"
                  tooltip="Web UI / API サーバーの待受アドレス（host:port）です。"
                  value={server.addr ?? ""}
                  placeholder="127.0.0.1:47950"
                  onCommit={(v) => patchSection<ServerSectionLocal>("server", { addr: v })}
                />
                <ToggleField
                  label="起動時にブラウザを自動で開く"
                  checked={server.auto_open ?? false}
                  onChange={(v) => patchSection<ServerSectionLocal>("server", { auto_open: v })}
                />
              </div>
            </section>
          )}

          {activeTab === "json" && (
            <section class="settings-section">
              <JsonEditor />
            </section>
          )}
        </>
      )}
    </div>
  );
}
