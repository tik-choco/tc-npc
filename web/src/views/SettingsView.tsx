// 設定 tab: structured forms over the server config document (GET/PUT
// /api/config), following the tik-choco suite's shared LLM-settings
// convention (tc-docs/drafts/llm-settings-common-v1.md §3): 接続先
// (provider) と モデル (preset) はそれぞれ独立したカード一覧で管理し、各タスク
// (会話/記憶/埋め込み/視覚/行動/通訳/音声合成/音声認識) は自分の preset を選ぶ
// だけ — api.base_url / api.model を直接編集する平坦なフォームはもう無い。旧
// 「音声」タブは廃止し、TTS/STT の有効トグルは モジュール・連携 タブへ、モデル
// 選択は タスク タブの行へ、それぞれ移した。
//
// Tabs, in this order: 一般 (display language / NPC reply language) → AI接続
// (ProviderCards + PresetCards, from lib/llm-config.ts's provider/preset
// model) → タスク (TaskRows: per-task preset assignment) → AI Network
// (NetworkSettings: room-based LLM sharing) → 通訳 (mode + languages/context
// size/auto_reverse/chatbox — model selection lives in the タスク tab now) →
// モジュール・連携 (feature on/off incl. 音声合成/音声認識 + VRC + server) →
// 詳細 (raw JSON, outside autosave).
//
// Field primitives (TextField/ToggleField/SelectField/ReasoningEffortField/
// RestartBadge/ModelPicker) live in components/SettingsFields.tsx and are
// shared with the provider/preset card UI — this view no longer keeps its
// own copies. Persistence is silent autosave via useConfigDoc — there is no
// save button except on the raw-JSON tab, which is intentionally outside
// the autosave path (see its section below).
//
// Reflection timing differs by field: schedule/location/route edits (owned
// by the 予定/行動 tabs, not this view) apply live; everything edited here
// (AI接続, module on/off, VRC/server settings) only takes effect after the
// app restarts. That's noted with a small badge per section rather than a
// paragraph, per the shared convention's "説明は最小限、詳細はツールチップへ"
// principle. The one exception is the 一般 tab's display language, which is
// browser-local and switches instantly — it carries no restart badge for
// exactly that reason.
//
// Config sections are typed via lib/config-types.ts where a shared type
// exists (tts/stt/vrc/enabled-only sections, plus providers[]/presets[] via
// lib/llm-config.ts); `server` has no shared type (only the
// schedule/action-map/settings views touch config, and none of the others
// need `server`), so a small local interface covers it here. Every write
// spreads the existing section object before applying a patch, so fields
// this view doesn't know about survive the round-trip.
import { useEffect, useState } from "preact/hooks";
import { Blocks, Braces, Cpu, Globe, Languages, ListChecks, Loader2, Network, RotateCcw, Save } from "lucide-preact";
import { getConfig, putConfig } from "../lib/api";
import { useConfigDoc } from "../hooks/useConfigDoc";
import { useI18n } from "../hooks/useI18n";
import { LANGS, type Lang, type MessageKey, type Translate } from "../lib/i18n";
import { SaveChip } from "../components/SaveChip";
import {
  InterpretModeField,
  InterpretScopeField,
  InterpretSettings,
  readTranslation,
  translationUpdater,
} from "../components/InterpretSettings";
import { RestartBadge, SelectField, TextField, ToggleField } from "../components/SettingsFields";
import { ProviderCards } from "../components/ProviderCards";
import { PresetCards } from "../components/PresetCards";
import { TaskRows } from "../components/TaskRows";
import { NetworkSettings } from "../components/NetworkSettings";
import type { EnabledSection, SpeechEndpointSection, VrcSection } from "../lib/config-types";
import type { ConfigDocument } from "../lib/types";
import "../styles/components.css";
import "../styles/settings.css";
import "../styles/interpret.css";

/** config.server — not in lib/config-types.ts (no other view touches it). */
interface ServerSectionLocal {
  addr?: string;
  auto_open?: boolean;
  [key: string]: unknown;
}

function section<T>(config: ConfigDocument | null, key: string): T {
  return ((config?.[key] as T | undefined) ?? ({} as T));
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

type SettingsTabId = "general" | "connection" | "tasks" | "network" | "interpret" | "modules" | "json";

const SETTINGS_TABS: Array<{ id: SettingsTabId; labelKey: MessageKey; icon: typeof Cpu }> = [
  { id: "general", labelKey: "settings.tab.general", icon: Globe },
  { id: "connection", labelKey: "settings.tab.connection", icon: Cpu },
  { id: "tasks", labelKey: "settings.tab.tasks", icon: ListChecks },
  { id: "network", labelKey: "settings.tab.network", icon: Network },
  { id: "interpret", labelKey: "settings.tab.interpret", icon: Languages },
  { id: "modules", labelKey: "settings.tab.modules", icon: Blocks },
  { id: "json", labelKey: "settings.tab.json", icon: Braces },
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

  const tts = section<SpeechEndpointSection>(config, "tts");
  const stt = section<SpeechEndpointSection>(config, "stt");
  const talk = section<EnabledSection>(config, "talk");
  const memory = section<EnabledSection>(config, "memory");
  const vision = section<EnabledSection>(config, "vision");
  const action = section<EnabledSection>(config, "action");
  const scheduler = section<EnabledSection>(config, "scheduler");
  const vrc = section<VrcSection>(config, "vrc");
  const server = section<ServerSectionLocal>(config, "server");
  const translation = readTranslation(config);
  const update = translationUpdater(mutate);

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

          {activeTab === "connection" && config && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>{t("settings.connection.title")}</h3>
                <RestartBadge t={t} />
              </div>
              <ProviderCards t={t} config={config} mutate={mutate} />
              <PresetCards t={t} config={config} mutate={mutate} />
            </section>
          )}

          {activeTab === "tasks" && config && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>{t("settings.tasks.title")}</h3>
                <RestartBadge t={t} />
              </div>
              <div class="settings-card">
                <TaskRows t={t} config={config} mutate={mutate} />
              </div>
            </section>
          )}

          {activeTab === "network" && config && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>{t("settings.network.title")}</h3>
                <RestartBadge t={t} />
              </div>
              <div class="settings-card">
                <NetworkSettings t={t} config={config} mutate={mutate} />
              </div>
            </section>
          )}

          {activeTab === "interpret" && (
            <section class="settings-section">
              <div class="settings-section-head">
                <h3>{t("interpret.title")}</h3>
              </div>
              <p class="interpret-hint">{t("interpret.hint")}</p>

              <div class="settings-card">
                {/* InterpretModeField + InterpretSettings are the SAME
                    components rendered in the チャット sidebar's 通訳 panel
                    (on purpose): the sidebar mounts just the mode field for a
                    quick on/off switch during a conversation, while this tab
                    mounts the whole form for full configuration. The two are
                    never mounted at the same time — app.tsx renders a single
                    top-level tab at once — so there's no write race between
                    them. Do not "deduplicate" this away. Model selection is
                    NOT part of this form anymore — it moved to the タスク
                    tab's 通訳 row, alongside every other task's model. */}
                <InterpretModeField translation={translation} update={update} />
                <InterpretScopeField translation={translation} update={update} />
                <InterpretSettings translation={translation} update={update} />
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
                  {/* TTS/STT used to have their own "音声" tab with fallback
                      base_url/api_key/model/voice fields; those fields moved
                      to provider cards (connection) and the タスク tab (model
                      assignment), so only the on/off switch is left here.

                      These two are the section's exceptions to the restart
                      badge above: npc-speech runs regardless and starts or
                      stops its capture/playback threads on the config update,
                      so the switch takes effect at once. Said in a tooltip
                      rather than a badge — a per-toggle badge inside the grid
                      would crowd the row for a detail you only look for once. */}
                  <ToggleField
                    label={t("settings.module.tts")}
                    tooltip={t("settings.module.speech.live")}
                    checked={tts.enabled ?? false}
                    onChange={(v) => patchSection<SpeechEndpointSection>("tts", { enabled: v })}
                  />
                  <ToggleField
                    label={t("settings.module.stt")}
                    tooltip={t("settings.module.speech.live")}
                    checked={stt.enabled ?? false}
                    onChange={(v) => patchSection<SpeechEndpointSection>("stt", { enabled: v })}
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
