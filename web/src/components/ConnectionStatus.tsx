import type { ConnectionState } from "../lib/ws";
import "../styles/components.css";

const LABELS: Record<ConnectionState, string> = {
  open: "接続中",
  connecting: "接続待機中",
  closed: "切断",
};

export function ConnectionStatus({ state }: { state: ConnectionState }) {
  return (
    <span class={`chip chip--${state}`}>
      <span class="chip-dot" />
      {LABELS[state]}
    </span>
  );
}
