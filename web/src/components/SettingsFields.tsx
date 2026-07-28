// Shared field primitives for the 設定 tab (views/SettingsView.tsx) and the
// provider/preset card UI built on lib/llm-config.ts. Split out of
// SettingsView so both can use the same blur-commit text field, toggle,
// select, model/voice picker, and reasoning-effort control without
// duplicating them.
//
// Reflection/persistence conventions (see SettingsView.tsx's file header for
// the full rationale) carry over unchanged: onInput only updates local draft
// state, onCommit/onChange is what actually writes back, and a model/voice
// list fetch doubles as the connection test.
import type { ComponentChildren } from "preact";
import { useEffect, useMemo, useState } from "preact/hooks";
import { RefreshCw } from "lucide-preact";
import { listModels, listVoices } from "../lib/api";
import type { MessageKey, Translate } from "../lib/i18n";

export function commitOnEnter(event: KeyboardEvent): void {
  if (event.key === "Enter") (event.currentTarget as HTMLElement).blur();
}

// --- Generic blur-commit text field ---------------------------------------
// onInput only updates local draft state; the edit is written back to the
// config document (via onCommit -> mutate) on blur, or on Enter (which just
// triggers blur). This matches the tc-town reference implementation and
// avoids a PUT per keystroke.

export function TextField(props: {
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

export function ToggleField(props: { label: string; tooltip?: string; checked: boolean; onChange: (v: boolean) => void }) {
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
export function SelectField(props: {
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

/** Shared llm-settings union (tc-docs/drafts/llm-settings-common-v1.md §2.3).
 * "none" is an explicit value that is always sent, not "don't send". */
export const REASONING_EFFORT_OPTIONS: Array<{ value: string; hintKey: MessageKey }> = [
  { value: "none", hintKey: "settings.effort.none" },
  { value: "minimal", hintKey: "settings.effort.minimal" },
  { value: "low", hintKey: "settings.effort.low" },
  { value: "medium", hintKey: "settings.effort.medium" },
  { value: "high", hintKey: "settings.effort.high" },
];

/** Same scale, for a preset's reasoning_effort field: presets can also
 * inherit config.api's effort by leaving this at "" (lib/llm-config.ts's
 * PresetEntry.reasoning_effort), so this adds an "inherit" option in front.
 * Options display their own value string as the label (same as the
 * segmented control below), not a translated "settings.effort.<v>" string. */
export const PRESET_EFFORT_OPTIONS: Array<{ value: string; labelKey: MessageKey }> = [
  { value: "", labelKey: "llm.effort.inherit" },
  ...REASONING_EFFORT_OPTIONS.map((opt) => ({ value: opt.value, labelKey: opt.hintKey })),
];

/** 推論エフォート — a 5-step ordinal scale, so it's a segmented control rather
 * than a <select>: all steps stay visible and the hint line explains the one
 * that's picked. Not a <label> wrapper (a radiogroup isn't a single control);
 * the radios carry the semantics and give arrow-key navigation for free.
 * `name` defaults to the original single-instance radio group name; pass a
 * distinct one when multiple of these render on the same screen (e.g. one
 * per preset card) so their radios don't fight each other. */
export function ReasoningEffortField(props: {
  t: Translate;
  value: string;
  onChange: (value: string) => void;
  name?: string;
}) {
  const { t, value, onChange, name = "api-reasoning-effort" } = props;
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
              name={name}
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
export function RestartBadge({ t }: { t: Translate }) {
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

export type PickerKind = "models" | "voices";
export type PickerStatus = "idle" | "loading" | "error" | "done";

export function ModelPicker(props: {
  t: Translate;
  value: string;
  placeholder: string;
  baseUrl: string;
  apiKey: string;
  section: "api" | "tts" | "stt";
  kind: PickerKind;
  itemLabel: string;
  onChange: (value: string) => void;
  /** When apiKey is the masked "***" sentinel, the server looks up the
   * saved key for this provider id instead of using the literal string.
   * Passed through to POST /api/llm/models|voices. */
  providerId?: string;
}) {
  const { t, value, placeholder, baseUrl, apiKey, section: sectionName, kind, itemLabel, onChange, providerId } = props;
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
      const req = { baseUrl, apiKey, section: sectionName, providerId };
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
