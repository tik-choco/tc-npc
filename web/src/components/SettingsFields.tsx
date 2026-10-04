// General settings fields; AI controls use the REST-backed mistai companion.
import type { ComponentChildren } from "preact";
import { useEffect, useState } from "preact/hooks";
import type { Translate } from "../lib/i18n";

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

/** Small "反映は再起動後" marker — a tooltip-only badge, not a hint paragraph. */
export function RestartBadge({ t }: { t: Translate }) {
  return (
    <span class="badge settings-restart-badge" title={t("settings.restart.tooltip")}>
      {t("settings.restart")}
    </span>
  );
}
