// チャット tab: merged transcript (user/assistant bubbles, sense/memory/
// action/error entries rendered as muted system lines) + composer + interrupt.
import { useMemo, useState } from "preact/hooks";
import { AlertTriangle, Eye, Ear, Brain, Activity, SendHorizontal, OctagonX } from "lucide-preact";
import type { ErrorEntry, TimelineEntry } from "../hooks/useNpcSocket";
import { useAutoScroll } from "../hooks/useAutoScroll";
import type { ConnectionState } from "../lib/ws";
import { ConnectionStatus } from "../components/ConnectionStatus";
import { useI18n } from "../hooks/useI18n";
import type { Translate } from "../lib/i18n";
import "../styles/components.css";
import "../styles/chat.css";

/** An `ErrorEntry` reshaped to slot into the transcript alongside
 *  `TimelineEntry`, ordered by the same monotonic id sequence. */
type ErrorRow = { id: number; kind: "error"; text: string; ts: number };
type Row = TimelineEntry | ErrorRow;

function formatTime(ts: number): string {
  if (!ts) return "";
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function SystemLine({ entry, t }: { entry: Exclude<Row, { kind: "chat" }>; t: Translate }) {
  if (entry.kind === "sense") {
    const Icon = entry.senseKind === "vision" ? Eye : Ear;
    return (
      <div class="chat-system-line">
        <Icon size={13} aria-hidden="true" />
        <span>{entry.text}</span>
        <time>{formatTime(entry.ts)}</time>
      </div>
    );
  }
  if (entry.kind === "memory") {
    return (
      <div class="chat-system-line">
        <Brain size={13} aria-hidden="true" />
        <span>
          [{entry.memoryKind === "short" ? t("chat.memory.short") : t("chat.memory.long")}] {entry.text}
        </span>
        <time>{formatTime(entry.ts)}</time>
      </div>
    );
  }
  if (entry.kind === "error") {
    return (
      <div class="chat-system-line chat-system-line--error">
        <AlertTriangle size={13} aria-hidden="true" />
        <span>
          [{t("chat.error")}] {entry.text}
        </span>
        <time>{formatTime(entry.ts)}</time>
      </div>
    );
  }
  return (
    <div class="chat-system-line">
      <Activity size={13} aria-hidden="true" />
      <span>{entry.text}</span>
      <time>{formatTime(entry.ts)}</time>
    </div>
  );
}

function TypingIndicator({ t }: { t: Translate }) {
  return (
    <div class="chat-typing" role="status" aria-label={t("chat.typing")}>
      <span class="chat-typing-dot" />
      <span class="chat-typing-dot" />
      <span class="chat-typing-dot" />
    </div>
  );
}

// Persists the "who am I" name across reloads, mirroring the localStorage
// pattern used by i18n's LANG_KEY. Kept separate from server state: this is
// purely a client-side convenience so the user doesn't retype it every time.
const SPEAKER_KEY = "tc-npc:speaker";

function loadSpeaker(): string {
  try {
    return localStorage.getItem(SPEAKER_KEY) ?? "";
  } catch {
    return "";
  }
}

function saveSpeaker(value: string): void {
  try {
    localStorage.setItem(SPEAKER_KEY, value);
  } catch {
    // Ignore storage failures (private browsing, etc).
  }
}

export interface ChatViewProps {
  entries: TimelineEntry[];
  errors: ErrorEntry[];
  pending: boolean;
  connectionState: ConnectionState;
  onSend: (text: string, speaker?: string) => void;
  onInterrupt: () => void;
}

export function ChatView({ entries, errors, pending, connectionState, onSend, onInterrupt }: ChatViewProps) {
  const { t } = useI18n();
  const [draft, setDraft] = useState("");
  const [speaker, setSpeaker] = useState(loadSpeaker);
  const connected = connectionState === "open";

  // Errors share the same monotonic id sequence as timeline entries (see
  // useNpcSocket.ts), so merging and sorting by id keeps everything in
  // arrival order.
  const rows = useMemo<Row[]>(() => {
    const errorRows: ErrorRow[] = errors.map((e) => ({ id: e.id, kind: "error", text: e.message, ts: e.ts }));
    return [...entries, ...errorRows].sort((a, b) => a.id - b.id);
  }, [entries, errors]);

  const lastId = rows.length > 0 ? rows[rows.length - 1].id : undefined;
  // Keyed on the newest row's id (not row count): the underlying lists are
  // capped (MAX_ENTRIES in useNpcSocket.ts), so once capped the count stops
  // changing forever even as new entries keep replacing old ones. The
  // pending flag is folded in too, so the typing indicator's appearance/
  // disappearance also triggers the near-bottom follow.
  const scrollRef = useAutoScroll(`${lastId ?? "none"}:${pending}`);

  function updateSpeaker(value: string) {
    setSpeaker(value);
    saveSpeaker(value);
  }

  function submit() {
    const text = draft.trim();
    if (!text || !connected) return;
    const name = speaker.trim();
    onSend(text, name || undefined);
    setDraft("");
  }

  return (
    <div class="chat-view">
      <div class="chat-toolbar">
        <ConnectionStatus state={connectionState} />
        <button type="button" class="btn btn-danger btn-small" onClick={onInterrupt}>
          <OctagonX size={14} />
          {t("chat.interrupt")}
        </button>
      </div>

      <div class="chat-transcript" ref={scrollRef} role="log" aria-live="polite">
        {rows.length === 0 && !pending && (
          <div class="empty-state">
            <div class="empty-state-title">{t("chat.empty.title")}</div>
            <div class="empty-state-description">{t("chat.empty.desc")}</div>
          </div>
        )}
        {rows.map((row) =>
          row.kind === "chat" ? (
            <div key={row.id} class={`chat-bubble-row chat-bubble-row--${row.role}`}>
              <div class={`chat-bubble chat-bubble--${row.role}`}>
                <div class="chat-bubble-text">{row.text}</div>
                <time>{formatTime(row.ts)}</time>
              </div>
            </div>
          ) : (
            <SystemLine key={row.id} entry={row} t={t} />
          ),
        )}
        {pending && <TypingIndicator t={t} />}
      </div>

      <div class="chat-composer">
        <input
          type="text"
          class="chat-speaker-input"
          placeholder={t("chat.speaker.placeholder")}
          aria-label={t("chat.speaker.aria")}
          value={speaker}
          onInput={(e) => updateSpeaker((e.target as HTMLInputElement).value)}
        />
        <input
          type="text"
          placeholder={connected ? t("chat.placeholder") : t("chat.placeholder.disconnected")}
          value={draft}
          disabled={!connected}
          onInput={(e) => setDraft((e.target as HTMLInputElement).value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && !e.isComposing) {
              e.preventDefault();
              submit();
            }
          }}
        />
        <button type="button" class="btn btn-primary" onClick={submit} disabled={!connected || !draft.trim()}>
          <SendHorizontal size={16} />
          {t("common.send")}
        </button>
      </div>
    </div>
  );
}
