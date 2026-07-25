import type { SaveState } from "../hooks/useConfigDoc";
import type { MessageKey } from "../lib/i18n";
import { useI18n } from "../hooks/useI18n";
import "../styles/components.css";

// The 設定/予定/行動 views all write config through useConfigDoc and all
// showed the same save indicator — in three wordings and three shades. This is
// that indicator: same shape as <ConnectionStatus> (chip + dot), with the
// error detail in a tooltip rather than inline, so a long message can't push
// the header around.

const LABEL_KEYS: Record<Exclude<SaveState, "idle">, MessageKey> = {
  saving: "save.saving",
  saved: "save.saved",
  error: "save.error",
};

export function SaveChip({ state, error }: { state: SaveState; error?: string | null }) {
  const { t } = useI18n();
  if (state === "idle") return null;
  return (
    <span class={`chip chip--${state}`} title={state === "error" ? (error ?? undefined) : undefined}>
      <span class="chip-dot" />
      {t(LABEL_KEYS[state])}
    </span>
  );
}
