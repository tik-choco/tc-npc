import type { ConnectionState } from "../lib/ws";
import type { MessageKey } from "../lib/i18n";
import { useI18n } from "../hooks/useI18n";
import "../styles/components.css";

const LABEL_KEYS: Record<ConnectionState, MessageKey> = {
  open: "conn.open",
  connecting: "conn.connecting",
  closed: "conn.closed",
};

export function ConnectionStatus({ state }: { state: ConnectionState }) {
  const { t } = useI18n();
  return (
    <span class={`chip chip--${state}`}>
      <span class="chip-dot" />
      {t(LABEL_KEYS[state])}
    </span>
  );
}
