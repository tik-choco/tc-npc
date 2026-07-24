// 行動 tab: position readout, a free-form command composer, the action log,
// and a stop button (sends `command` "stop").
import { useEffect, useRef, useState } from "preact/hooks";
import { Compass, ListTree, Send, Square } from "lucide-preact";
import type { ActionLogEntry, PositionState } from "../hooks/useNpcSocket";
import "../styles/components.css";
import "../styles/action.css";

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(
    d.getSeconds(),
  ).padStart(2, "0")}`;
}

export interface ActionViewProps {
  position: PositionState | null;
  actionLogEntries: ActionLogEntry[];
  onCommand: (text: string) => void;
}

export function ActionView({ position, actionLogEntries, onCommand }: ActionViewProps) {
  const [draft, setDraft] = useState("");
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [actionLogEntries.length]);

  function submit() {
    const text = draft.trim();
    if (!text) return;
    onCommand(text);
    setDraft("");
  }

  return (
    <div class="action-view">
      <section class="action-position-card">
        <h2 class="action-section-title">
          <Compass size={16} />
          位置情報
        </h2>
        {position ? (
          <div class="action-position-grid">
            <div>
              <span class="action-position-label">X</span>
              <span class="action-position-value">{position.x.toFixed(2)}</span>
            </div>
            <div>
              <span class="action-position-label">Y</span>
              <span class="action-position-value">{position.y.toFixed(2)}</span>
            </div>
            <div>
              <span class="action-position-label">向き</span>
              <span class="action-position-value">{position.heading.toFixed(1)}°</span>
            </div>
          </div>
        ) : (
          <div class="action-position-empty">位置情報は未受信です</div>
        )}
      </section>

      <section class="action-command-card">
        <h2 class="action-section-title">
          <Send size={16} />
          コマンド送信
        </h2>
        <div class="action-command-row">
          <input
            type="text"
            placeholder="mv 3 / rt 90 / goto 5 10 / route <name>"
            value={draft}
            onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
            onKeyDown={(e) => {
              if (e.key === "Enter" && !e.isComposing) {
                e.preventDefault();
                submit();
              }
            }}
          />
          <button type="button" class="btn btn-primary" onClick={submit} disabled={!draft.trim()}>
            <Send size={14} />
            送信
          </button>
          <button type="button" class="btn btn-danger" onClick={() => onCommand("stop")}>
            <Square size={14} />
            停止
          </button>
        </div>
      </section>

      <section class="action-log-section">
        <h2 class="action-section-title">
          <ListTree size={16} />
          行動ログ
        </h2>
        <div class="action-log-scroll" ref={scrollRef}>
          {actionLogEntries.length === 0 && (
            <div class="empty-state">
              <div class="empty-state-title">まだ行動ログがありません</div>
            </div>
          )}
          {actionLogEntries.map((entry) => (
            <div key={entry.id} class="action-log-item">
              <time>{formatTime(entry.ts)}</time>
              <span>{entry.text}</span>
            </div>
          ))}
        </div>
      </section>
    </div>
  );
}
