// チャット tab: merged transcript (user/assistant bubbles, sense/memory/
// action entries rendered as muted system lines) + composer + interrupt.
import { useEffect, useRef, useState } from "preact/hooks";
import { Eye, Ear, Brain, Activity, SendHorizontal, OctagonX } from "lucide-preact";
import type { TimelineEntry } from "../hooks/useNpcSocket";
import type { ConnectionState } from "../lib/ws";
import { ConnectionStatus } from "../components/ConnectionStatus";
import "../styles/components.css";
import "../styles/chat.css";

function formatTime(ts: number): string {
  if (!ts) return "";
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function SystemLine({ entry }: { entry: Exclude<TimelineEntry, { kind: "chat" }> }) {
  if (entry.kind === "sense") {
    const Icon = entry.senseKind === "vision" ? Eye : Ear;
    return (
      <div class="chat-system-line">
        <Icon size={13} />
        <span>{entry.text}</span>
        <time>{formatTime(entry.ts)}</time>
      </div>
    );
  }
  if (entry.kind === "memory") {
    return (
      <div class="chat-system-line">
        <Brain size={13} />
        <span>
          [{entry.memoryKind === "short" ? "短期記憶" : "長期記憶"}] {entry.text}
        </span>
        <time>{formatTime(entry.ts)}</time>
      </div>
    );
  }
  return (
    <div class="chat-system-line">
      <Activity size={13} />
      <span>{entry.text}</span>
      <time>{formatTime(entry.ts)}</time>
    </div>
  );
}

export interface ChatViewProps {
  entries: TimelineEntry[];
  connectionState: ConnectionState;
  onSend: (text: string) => void;
  onInterrupt: () => void;
}

export function ChatView({ entries, connectionState, onSend, onInterrupt }: ChatViewProps) {
  const [draft, setDraft] = useState("");
  const scrollRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const el = scrollRef.current;
    if (!el) return;
    el.scrollTop = el.scrollHeight;
  }, [entries.length]);

  function submit() {
    const text = draft.trim();
    if (!text) return;
    onSend(text);
    setDraft("");
  }

  return (
    <div class="chat-view">
      <div class="chat-toolbar">
        <ConnectionStatus state={connectionState} />
        <button type="button" class="btn btn-danger btn-small" onClick={onInterrupt}>
          <OctagonX size={14} />
          割り込み
        </button>
      </div>

      <div class="chat-transcript" ref={scrollRef}>
        {entries.length === 0 && (
          <div class="empty-state">
            <div class="empty-state-title">まだ会話がありません</div>
            <div class="empty-state-description">下のボックスからメッセージを送るとここに表示されます。</div>
          </div>
        )}
        {entries.map((entry) =>
          entry.kind === "chat" ? (
            <div key={entry.id} class={`chat-bubble-row chat-bubble-row--${entry.role}`}>
              <div class={`chat-bubble chat-bubble--${entry.role}`}>
                <div class="chat-bubble-text">{entry.text}</div>
                <time>{formatTime(entry.ts)}</time>
              </div>
            </div>
          ) : (
            <SystemLine key={entry.id} entry={entry} />
          ),
        )}
      </div>

      <div class="chat-composer">
        <input
          type="text"
          placeholder="メッセージを入力…"
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
          <SendHorizontal size={16} />
          送信
        </button>
      </div>
    </div>
  );
}
