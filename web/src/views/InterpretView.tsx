// 通訳 tab: simultaneous interpretation — the settings for config.translation
// plus the live transcript npc-translate publishes over the WS connection.
//
// Settings autosave through useConfigDoc like the 予定/行動 tabs, and the
// server hot-reloads them (mode, languages and context size all take effect
// on the next utterance), so there is no restart badge here. The transcript
// itself is push-only: entries arrive as the original line first and fill in
// each target language as its translation lands.
import { AlertTriangle, Languages, MessageSquare, Mic } from "lucide-preact";

import { useAutoScroll } from "../hooks/useAutoScroll";
import { useConfigDoc } from "../hooks/useConfigDoc";
import { useI18n } from "../hooks/useI18n";
import type { TranslationEntry } from "../hooks/useNpcSocket";
import { SaveChip } from "../components/SaveChip";
import type { MessageKey } from "../lib/i18n";
import type { TranslationSection } from "../lib/config-types";
import type { ConfigDocument } from "../lib/types";
import "../styles/components.css";
import "../styles/settings.css";
import "../styles/interpret.css";

/** config.translation.mode values, with the copy that explains each. */
const MODES: Array<{ value: string; labelKey: MessageKey; hintKey: MessageKey }> = [
  { value: "off", labelKey: "interpret.mode.off", hintKey: "interpret.mode.off.hint" },
  { value: "interpret", labelKey: "interpret.mode.interpret", hintKey: "interpret.mode.interpret.hint" },
  { value: "assist", labelKey: "interpret.mode.assist", hintKey: "interpret.mode.assist.hint" },
];

/** Suggestions for the language fields — free text, so anything else works
 *  too (the labels are handed to the LLM verbatim). */
const LANGUAGE_SUGGESTIONS = ["日本語", "英語", "中国語", "台湾華語", "韓国語", "スペイン語"];

function readTranslation(config: ConfigDocument): TranslationSection {
  return (config.translation as TranslationSection | undefined) ?? {};
}

function TranscriptEntry({ entry }: { entry: TranslationEntry }) {
  const { t } = useI18n();
  const langs = Object.keys(entry.translations);

  return (
    <div class={`interpret-entry interpret-entry--${entry.source}`}>
      <div class="interpret-entry-head">
        <span class="interpret-entry-source">
          {entry.source === "user" ? <Mic size={13} /> : <MessageSquare size={13} />}
          {t(entry.source === "user" ? "interpret.source.user" : "interpret.source.agent")}
        </span>
        {entry.reversed && <span class="badge interpret-reversed">{t("interpret.reversed")}</span>}
        <span class="interpret-entry-time">{new Date(entry.ts).toLocaleTimeString()}</span>
      </div>
      <p class="interpret-original">{entry.original}</p>
      {langs.length === 0 ? (
        <p class="interpret-pending">{t("interpret.pending")}</p>
      ) : (
        langs.map((lang) => (
          <p key={lang} class="interpret-translated">
            <span class="interpret-lang">{lang}</span>
            {entry.translations[lang]}
          </p>
        ))
      )}
    </div>
  );
}

export function InterpretView({ translations }: { translations: TranslationEntry[] }) {
  const { t } = useI18n();
  const { config, loadError, saveState, saveError, mutate } = useConfigDoc();
  // Keyed on the newest entry's id (not `.length`): the list is capped at
  // MAX_ENTRIES, so length alone would stop changing — and the effect would
  // stop firing — once the cap is hit.
  const transcriptRef = useAutoScroll(translations[translations.length - 1]?.id);

  function update(fn: (section: TranslationSection) => void) {
    mutate((draft) => {
      const section = ((draft as ConfigDocument).translation as TranslationSection | undefined) ?? {};
      fn(section);
      (draft as ConfigDocument).translation = section;
    });
  }

  if (loadError) {
    return (
      <div class="empty-state">
        <div class="empty-state-icon">
          <AlertTriangle size={24} />
        </div>
        <div class="empty-state-title">{t("interpret.loadError")}</div>
        <div class="empty-state-description">{loadError}</div>
      </div>
    );
  }

  if (!config) {
    return <div class="empty-state">{t("common.loading")}</div>;
  }

  const translation = readTranslation(config);
  const mode = translation.mode ?? "off";
  const activeMode = MODES.find((m) => m.value === mode) ?? MODES[0];
  const running = mode !== "off";

  return (
    <div class="interpret-view">
      <div class="interpret-header">
        <div class="interpret-header-title">
          <Languages size={18} />
          <h2>{t("interpret.title")}</h2>
          <SaveChip state={saveState} error={saveError} />
        </div>
      </div>

      <p class="interpret-hint">{t("interpret.hint")}</p>

      <section class="settings-card interpret-settings">
        <div class="field">
          <span>{t("interpret.mode")}</span>
          <div class="segmented">
            {MODES.map((option) => (
              <label
                key={option.value}
                class={`segmented-option${option.value === mode ? " is-active" : ""}`}
                title={t(option.hintKey)}
              >
                <input
                  type="radio"
                  name="translation-mode"
                  value={option.value}
                  checked={option.value === mode}
                  onChange={() =>
                    update((section) => {
                      section.mode = option.value;
                    })
                  }
                />
                {t(option.labelKey)}
              </label>
            ))}
          </div>
          <span class="field-hint">{t(activeMode.hintKey)}</span>
        </div>

        <datalist id="interpret-languages">
          {LANGUAGE_SUGGESTIONS.map((lang) => (
            <option key={lang} value={lang} />
          ))}
        </datalist>

        <div class="interpret-lang-grid">
          <label class="field" title={t("interpret.field.source.tooltip")}>
            <span>{t("interpret.field.source")}</span>
            <input
              type="text"
              list="interpret-languages"
              value={translation.source_language ?? ""}
              onChange={(e) =>
                update((section) => {
                  section.source_language = (e.target as HTMLInputElement).value;
                })
              }
            />
          </label>

          <label class="field">
            <span>{t("interpret.field.target")}</span>
            <input
              type="text"
              list="interpret-languages"
              value={translation.target_language ?? ""}
              onChange={(e) =>
                update((section) => {
                  section.target_language = (e.target as HTMLInputElement).value;
                })
              }
            />
          </label>

          <label class="field" title={t("interpret.field.target2.tooltip")}>
            <span>{t("interpret.field.target2")}</span>
            <input
              type="text"
              list="interpret-languages"
              value={translation.target_language_2 ?? ""}
              onChange={(e) =>
                update((section) => {
                  section.target_language_2 = (e.target as HTMLInputElement).value;
                })
              }
            />
          </label>

          <label class="field" title={t("interpret.field.contextSize.tooltip")}>
            <span>{t("interpret.field.contextSize")}</span>
            <input
              type="number"
              min={0}
              max={20}
              value={translation.context_size ?? 3}
              onChange={(e) =>
                update((section) => {
                  section.context_size = Number((e.target as HTMLInputElement).value);
                })
              }
            />
          </label>
        </div>

        <label class="field" title={t("interpret.field.model.tooltip")}>
          <span>{t("interpret.field.model")}</span>
          <input
            type="text"
            placeholder={t("interpret.field.model.placeholder")}
            value={translation.model ?? ""}
            onChange={(e) =>
              update((section) => {
                section.model = (e.target as HTMLInputElement).value;
              })
            }
          />
        </label>

        <div class="settings-toggle-grid">
          <label class="settings-toggle" title={t("interpret.field.autoReverse.tooltip")}>
            <input
              type="checkbox"
              checked={translation.auto_reverse ?? true}
              onChange={(e) =>
                update((section) => {
                  section.auto_reverse = (e.target as HTMLInputElement).checked;
                })
              }
            />
            <span>{t("interpret.field.autoReverse")}</span>
          </label>

          <label class="settings-toggle" title={t("interpret.field.chatbox.tooltip")}>
            <input
              type="checkbox"
              checked={translation.chatbox ?? false}
              onChange={(e) =>
                update((section) => {
                  section.chatbox = (e.target as HTMLInputElement).checked;
                })
              }
            />
            <span>{t("interpret.field.chatbox")}</span>
          </label>
        </div>
      </section>

      <section class="interpret-log">
        <div class="interpret-log-head">
          <h3>{t("interpret.log.title")}</h3>
          {!running && <span class="badge interpret-off-badge">{t("interpret.off.badge")}</span>}
        </div>
        <div class="interpret-transcript" ref={transcriptRef}>
          {translations.length === 0 ? (
            <p class="interpret-empty">{t("interpret.log.empty")}</p>
          ) : (
            translations.map((entry) => <TranscriptEntry key={entry.id} entry={entry} />)
          )}
        </div>
      </section>
    </div>
  );
}
