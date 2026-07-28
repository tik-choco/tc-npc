// Shared 通訳 (simultaneous interpretation) settings form. Previously this
// form was hand-rolled twice: once inside SettingsView's 通訳 tab, and the
// チャット sidebar's 通訳 panel only showed a hint pointing the user at that
// tab instead of letting them edit inline. Extracting it here lets both
// homes render the identical form — the sidebar keeps it available while a
// conversation is live, 設定 keeps the roomy full-page version.
//
// This component never touches useConfigDoc itself: each mounted tree may
// own at most one in-flight config editor (see useConfigDoc's header comment
// for why), so both call sites keep their own useConfigDoc() and hand this
// component the resulting `translation` slice plus a `translationUpdater`
// (below) wrapping their own `mutate`. That keeps the write path identical
// between the two homes without a second editor fighting the first.
//
// Field markup is hand-rolled with plain `<label class="field">` rather than
// reusing components/SettingsFields.tsx's TextField/ToggleField: the
// language inputs need a `list=` datalist attribute that TextField's props
// don't support, and it's not worth a one-off prop just for this. The
// text/number fields commit on native `change`
// (i.e. on blur, same as Enter-then-blur) rather than mirroring TextField's
// input-then-commit-on-blur draft state — for an uncontrolled-feeling text
// input the two are equivalent, and this matches how SettingsView already
// hand-rolled the source/target/target2/context_size fields before this
// extraction (see git history), so behavior is unchanged.
import { useI18n } from "../hooks/useI18n";
import type { ConfigDocHandle } from "../hooks/useConfigDoc";
import type { MessageKey } from "../lib/i18n";
import type { TranslationSection } from "../lib/config-types";
import type { ConfigDocument } from "../lib/types";
import "../styles/components.css";
import "../styles/settings.css";
import "../styles/interpret.css";

/** config.translation.mode values, with the copy that explains each. Single
 *  source now — both InterpretPanel (チャット sidebar) and SettingsView (設定
 *  › 通訳) import this instead of keeping their own copy. */
export const TRANSLATION_MODES: Array<{ value: string; labelKey: MessageKey; hintKey: MessageKey }> = [
  { value: "off", labelKey: "interpret.mode.off", hintKey: "interpret.mode.off.hint" },
  { value: "interpret", labelKey: "interpret.mode.interpret", hintKey: "interpret.mode.interpret.hint" },
  { value: "assist", labelKey: "interpret.mode.assist", hintKey: "interpret.mode.assist.hint" },
];

/** Suggestions for the language fields — free text, so anything else works
 *  too (the labels are handed to the LLM verbatim). Copied verbatim from the
 *  form's previous home in SettingsView. */
const LANGUAGE_SUGGESTIONS = ["日本語", "英語", "中国語", "台湾華語", "韓国語", "スペイン語"];

/** config.translation, or {} when the document has no such section yet. */
export function readTranslation(config: ConfigDocument | null): TranslationSection {
  return (config?.translation as TranslationSection | undefined) ?? {};
}

/** Wraps a useConfigDoc mutate() into a "edit the translation section"
 *  updater, so both call sites write through identical code: read the
 *  current section (or {} if absent), let the caller mutate it in place,
 *  then write it back onto the draft document. */
export function translationUpdater(
  mutate: ConfigDocHandle["mutate"],
): (fn: (section: TranslationSection) => void) => void {
  return (fn) => {
    mutate((draft) => {
      const section = ((draft as ConfigDocument).translation as TranslationSection | undefined) ?? {};
      fn(section);
      (draft as ConfigDocument).translation = section;
    });
  };
}

export interface InterpretFieldsProps {
  translation: TranslationSection;
  update: (fn: (section: TranslationSection) => void) => void;
  /** true = rendered in the 320px チャット sidebar (stacked single column,
   *  tighter type); false/omitted = the roomy 設定 view layout. */
  compact?: boolean;
}

/** The mode segmented control + the active mode's explanatory hint. Split
 *  out from the rest because the sidebar keeps it permanently visible while
 *  the other fields sit inside a collapsed <details>. */
export function InterpretModeField({ translation, update, compact }: InterpretFieldsProps) {
  const { t } = useI18n();
  const mode = translation.mode ?? "off";
  const activeMode = TRANSLATION_MODES.find((m) => m.value === mode) ?? TRANSLATION_MODES[0];

  return (
    <div class={`field${compact ? " interpret-fields--compact" : ""}`}>
      <span>{t("interpret.mode")}</span>
      {/* Hand-rolled: a radiogroup with a per-option tooltip plus an
          active-hint line below it, which neither a plain <select> nor a
          generic ToggleField can express. `name` is a fixed literal rather
          than something derived per-instance: two copies of this control
          are never mounted at once (app.tsx renders one top-level tab at a
          time, and the チャット sidebar's 通訳 panel only exists while that
          tab is active), so there's no risk of two radiogroups fighting
          over the same name. */}
      <div class="segmented" aria-label={t("interpret.panel.mode.aria")}>
        {TRANSLATION_MODES.map((option) => (
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
  );
}

/** Everything except mode: source/target/target2 languages, context size,
 *  auto_reverse, chatbox. Model selection is not part of this form — it
 *  moved to 設定 › タスク's 通訳 row (lib/llm-config.ts's per-task preset
 *  assignment), which both call sites (this tab and the チャット sidebar's
 *  通訳 panel) already point users at implicitly since neither ever showed a
 *  タスク-tab shortcut here — see SettingsView.tsx's 通訳 tab comment. */
export function InterpretSettings({ translation, update, compact }: InterpretFieldsProps) {
  const { t } = useI18n();

  const languageFields = (
    <>
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
    </>
  );

  return (
    <div class={`interpret-fields${compact ? " interpret-fields--compact" : ""}`}>
      {/* Rendered once here regardless of which home mounts this component;
          fine even if both homes were ever mounted together since duplicate
          datalist ids are harmless (the browser just uses the first), and in
          practice they never are (see the radiogroup comment above). */}
      <datalist id="interpret-languages">
        {LANGUAGE_SUGGESTIONS.map((lang) => (
          <option key={lang} value={lang} />
        ))}
      </datalist>

      {/* Non-compact keeps the 4-up grid the 設定 view has always used;
          compact drops the grid entirely so the fields fall back to the
          parent's single-column flex stack instead of auto-fit potentially
          still fitting two 160px columns in a 320px sidebar. */}
      {compact ? languageFields : <div class="interpret-lang-grid">{languageFields}</div>}

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
    </div>
  );
}
