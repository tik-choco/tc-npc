// Owns the single NpcSocket connection for the app's lifetime and fans server
// frames out into per-view state slices: a merged `timeline` for the chat
// view (chat + sense + memory + actionLog, in arrival order) plus dedicated
// lists/scalars for the views that only care about one frame kind (音声's
// ttsLines/volume, 視覚's visionLog, 行動's actionLogEntries/position, 通訳's
// translations).
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { NpcSocket, type ConnectionState } from "../lib/ws";
import { getChatHistory } from "../lib/api";
import type { ClientMessage, DriveState, PersonRecord, ServerMessage } from "../lib/types";

const MAX_ENTRIES = 500;
// Sparkline history for the 感情 tab: ~60 turns is plenty for a trend line
// without holding onto unbounded state.
const MAX_AFFECT_HISTORY = 60;

export type TimelineEntry =
  | { id: number; kind: "chat"; role: "user" | "assistant"; text: string; ts: number }
  | { id: number; kind: "sense"; senseKind: "vision" | "speech"; text: string; ts: number }
  | { id: number; kind: "memory"; memoryKind: "short" | "long"; text: string; ts: number }
  | { id: number; kind: "actionLog"; text: string; ts: number };

export interface TtsLineEntry {
  id: number;
  text: string;
  translations?: Record<string, string>;
  ts: number;
}

/**
 * One interpreted utterance for the 通訳 tab, assembled from the several
 * `translation` frames that share an id: the original arrives first and each
 * target language fills in `translations` as it lands.
 */
export interface TranslationEntry {
  id: string;
  source: "user" | "agent";
  original: string;
  translations: Record<string, string>;
  reversed: boolean;
  ts: number;
}

export interface SenseEntry {
  id: number;
  kind: "vision" | "speech";
  text: string;
  ts: number;
}

export interface ActionLogEntry {
  id: number;
  text: string;
  ts: number;
}

export interface ErrorEntry {
  id: number;
  message: string;
  ts: number;
}

export interface PositionState {
  x: number;
  y: number;
  heading: number;
}

/** One `affect` frame, as kept for both the "latest" snapshot and the
 *  rolling history used by the 感情 tab's sparkline. */
export interface AffectSnapshot {
  ts: number;
  familiarity: number;
  closing: boolean;
  inviteCaution: boolean;
  drives: DriveState[];
}

export interface UseNpcSocketResult {
  connectionState: ConnectionState;
  version: string | null;
  modules: Record<string, boolean>;
  character: { id: string; name: string } | null;
  timeline: TimelineEntry[];
  ttsLines: TtsLineEntry[];
  translations: TranslationEntry[];
  visionLog: SenseEntry[];
  actionLogEntries: ActionLogEntry[];
  position: PositionState | null;
  volume: number;
  errors: ErrorEntry[];
  /** True from the moment an `input`/`command` is acked (`inputAccepted`)
   *  until the resulting `chat` reply (or a failure) lands — drives the
   *  チャット typing indicator. */
  pending: boolean;
  /** Latest `affect` frame, or null before the first one arrives. */
  affect: AffectSnapshot | null;
  /** Rolling history of `affect` frames (oldest first, capped at
   *  MAX_AFFECT_HISTORY) for the 感情 tab's sparkline. */
  affectHistory: AffectSnapshot[];
  /** Bumped on every `memory` frame so the 感情 tab can refetch
   *  GET /api/memory without owning a socket subscription itself. */
  memoryVersion: number;
  /** Tracked people, upserted by id from `person`/`personDeleted` frames.
   *  Unlike the other lists this is a set, not a log, so it isn't capped. */
  people: PersonRecord[];
  /** Bumped on every `person`/`personDeleted` frame so the 人物 tab can
   *  refetch GET /api/people without owning a socket subscription itself. */
  peopleVersion: number;
  send: (msg: ClientMessage) => void;
}

function cap<T>(list: T[], max: number): T[] {
  return list.length > max ? list.slice(list.length - max) : list;
}

export function useNpcSocket(): UseNpcSocketResult {
  const [connectionState, setConnectionState] = useState<ConnectionState>("connecting");
  const [version, setVersion] = useState<string | null>(null);
  const [modules, setModules] = useState<Record<string, boolean>>({});
  const [character, setCharacter] = useState<{ id: string; name: string } | null>(null);
  const [timeline, setTimeline] = useState<TimelineEntry[]>([]);
  const [ttsLines, setTtsLines] = useState<TtsLineEntry[]>([]);
  const [translations, setTranslations] = useState<TranslationEntry[]>([]);
  const [visionLog, setVisionLog] = useState<SenseEntry[]>([]);
  const [actionLogEntries, setActionLogEntries] = useState<ActionLogEntry[]>([]);
  const [position, setPosition] = useState<PositionState | null>(null);
  const [volume, setVolume] = useState(0);
  const [errors, setErrors] = useState<ErrorEntry[]>([]);
  const [pending, setPending] = useState(false);
  const [affect, setAffect] = useState<AffectSnapshot | null>(null);
  const [affectHistory, setAffectHistory] = useState<AffectSnapshot[]>([]);
  const [memoryVersion, setMemoryVersion] = useState(0);
  const [people, setPeople] = useState<PersonRecord[]>([]);
  const [peopleVersion, setPeopleVersion] = useState(0);

  const socketRef = useRef<NpcSocket | null>(null);
  const seqRef = useRef(0);

  const send = useCallback((msg: ClientMessage) => {
    socketRef.current?.send(msg);
  }, []);

  useEffect(() => {
    let cancelled = false;

    function nextId(): number {
      seqRef.current += 1;
      return seqRef.current;
    }

    // Backfill the transcript from the server's chat history on first mount
    // only (empty deps below). Ids are negative and assigned in chronological
    // order, so history rows always sort before every live entry (which uses
    // nextId()'s positive, ever-increasing sequence) regardless of whether
    // this fetch resolves before or after live `chat` frames start arriving
    // — see ChatView's id-sort. Prepending onto the latest `prev` (rather
    // than overwriting) is what avoids losing any such frames.
    async function loadHistory() {
      try {
        const { entries: history } = await getChatHistory(200);
        if (cancelled || history.length === 0) return;
        const rows: TimelineEntry[] = [];
        for (const entry of history) {
          const parsed = Date.parse(entry.time);
          const ts = Number.isNaN(parsed) ? Date.now() : parsed;
          if (entry.input) rows.push({ id: 0, kind: "chat", role: "user", text: entry.input, ts });
          if (entry.output) rows.push({ id: 0, kind: "chat", role: "assistant", text: entry.output, ts });
        }
        const base = -rows.length;
        rows.forEach((row, i) => {
          row.id = base + i;
        });
        setTimeline((prev) => {
          // Reload race: if a reply finishes between the history GET firing
          // and it resolving, the same turn lands both live (`chat` frame,
          // already in `prev`) and in `history` (chat-log.jsonl, written
          // right after the frame goes out) — see bus_forward.rs /
          // engine.rs. Drop history rows that duplicate a live one by
          // role+text so it doesn't render twice.
          //
          // Only the trailing edge of `rows` (the most recent history) can
          // plausibly collide with something already live in `prev` — that
          // race only touches the newest turn(s). Matching against `prev` in
          // full but restricting *which history rows* are eligible to a
          // small tail window keeps this from eating a legitimate repeat
          // buried earlier in the transcript (e.g. two separate "うん"
          // turns minutes apart).
          const TAIL_CHECK = 8;
          const liveChatKeys = new Set(prev.flatMap((e) => (e.kind === "chat" ? [`${e.role}:${e.text}`] : [])));
          const tailStart = rows.length - TAIL_CHECK;
          const deduped = rows.filter((row, i) => {
            if (i < tailStart) return true;
            return row.kind !== "chat" || !liveChatKeys.has(`${row.role}:${row.text}`);
          });
          return cap([...deduped, ...prev], MAX_ENTRIES);
        });
      } catch (err) {
        console.error("failed to load chat history", err);
      }
    }
    void loadHistory();

    function handleMessage(msg: ServerMessage) {
      const now = Date.now();
      switch (msg.type) {
        case "hello":
          setVersion(msg.version);
          setModules(msg.modules);
          setCharacter(msg.character);
          break;

        case "status":
          setModules(msg.modules);
          break;

        case "chat":
          setTimeline((prev) =>
            cap([...prev, { id: nextId(), kind: "chat", role: msg.role, text: msg.text, ts: msg.ts }], MAX_ENTRIES),
          );
          if (msg.role === "assistant") setPending(false);
          break;

        case "sense":
          setTimeline((prev) =>
            cap(
              [...prev, { id: nextId(), kind: "sense", senseKind: msg.kind, text: msg.text, ts: msg.ts }],
              MAX_ENTRIES,
            ),
          );
          if (msg.kind === "vision") {
            setVisionLog((prev) => cap([...prev, { id: nextId(), kind: "vision", text: msg.text, ts: msg.ts }], MAX_ENTRIES));
          }
          break;

        case "memory":
          setTimeline((prev) =>
            cap([...prev, { id: nextId(), kind: "memory", memoryKind: msg.kind, text: msg.text, ts: now }], MAX_ENTRIES),
          );
          // No payload to store here — just a signal for the 感情 tab to
          // refetch GET /api/memory.
          setMemoryVersion((v) => v + 1);
          break;

        case "affect": {
          const snapshot: AffectSnapshot = {
            ts: msg.ts,
            familiarity: msg.familiarity,
            closing: msg.closing,
            inviteCaution: msg.inviteCaution,
            drives: msg.drives,
          };
          setAffect(snapshot);
          setAffectHistory((prev) => cap([...prev, snapshot], MAX_AFFECT_HISTORY));
          break;
        }

        case "actionLog":
          setTimeline((prev) => cap([...prev, { id: nextId(), kind: "actionLog", text: msg.text, ts: now }], MAX_ENTRIES));
          setActionLogEntries((prev) => cap([...prev, { id: nextId(), text: msg.text, ts: now }], MAX_ENTRIES));
          break;

        case "ttsLine":
          setTtsLines((prev) =>
            cap([...prev, { id: nextId(), text: msg.text, translations: msg.translations, ts: now }], MAX_ENTRIES),
          );
          break;

        case "translation":
          // Upsert by id: the `lang: ""` frame creates the entry, each
          // later frame fills in one target language.
          setTranslations((prev) => {
            const index = prev.findIndex((entry) => entry.id === msg.id);
            if (index === -1) {
              return cap(
                [
                  ...prev,
                  {
                    id: msg.id,
                    source: msg.source,
                    original: msg.original,
                    translations: msg.lang === "" ? {} : { [msg.lang]: msg.text },
                    reversed: msg.reversed,
                    ts: msg.ts,
                  },
                ],
                MAX_ENTRIES,
              );
            }
            if (msg.lang === "") return prev;
            const next = [...prev];
            next[index] = {
              ...next[index],
              translations: { ...next[index].translations, [msg.lang]: msg.text },
            };
            return next;
          });
          break;

        case "position":
          setPosition({ x: msg.x, y: msg.y, heading: msg.heading });
          break;

        case "volume":
          setVolume(msg.level);
          break;

        case "error":
          setErrors((prev) => cap([...prev, { id: nextId(), message: msg.message, ts: now }], MAX_ENTRIES));
          setPending(false);
          break;

        case "response":
          if (msg.status === "error") {
            setErrors((prev) => cap([...prev, { id: nextId(), message: msg.message ?? "unknown error", ts: now }], MAX_ENTRIES));
          }
          setPending(false);
          break;

        case "person":
          // Upsert by id: replace the matching entry, or append if this is
          // a newly-seen person.
          setPeople((prev) => {
            const index = prev.findIndex((p) => p.id === msg.person.id);
            if (index === -1) return [...prev, msg.person];
            const next = [...prev];
            next[index] = msg.person;
            return next;
          });
          setPeopleVersion((v) => v + 1);
          break;

        case "personDeleted":
          setPeople((prev) => prev.filter((p) => p.id !== msg.id));
          setPeopleVersion((v) => v + 1);
          break;

        case "inputAccepted":
          // Server-side ack of a queued `input`/`command`. Drives the チャット
          // typing indicator; the resulting `chat`/`actionLog` frame is what
          // actually renders the reply and clears it again.
          setPending(true);
          break;

        default:
          break;
      }
    }

    const socket = new NpcSocket({
      onMessage: handleMessage,
      onStateChange: (state) => {
        setConnectionState(state);
        // A drop mid-response means no `chat`/`error`/`response` frame is
        // coming; don't leave the typing indicator stuck forever.
        if (state !== "open") setPending(false);
      },
    });
    socketRef.current = socket;
    socket.connect();

    return () => {
      cancelled = true;
      socket.close();
      socketRef.current = null;
    };
  }, []);

  return {
    connectionState,
    version,
    modules,
    character,
    timeline,
    ttsLines,
    translations,
    visionLog,
    actionLogEntries,
    position,
    volume,
    errors,
    pending,
    affect,
    affectHistory,
    memoryVersion,
    people,
    peopleVersion,
    send,
  };
}
