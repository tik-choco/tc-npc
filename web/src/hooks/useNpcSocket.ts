// Owns the single NpcSocket connection for the app's lifetime and fans server
// frames out into per-view state slices: a merged `timeline` for the chat
// view (chat + sense + memory + actionLog, in arrival order) plus dedicated
// lists/scalars for the views that only care about one frame kind (音声's
// ttsLines/volume, 視覚's visionLog, 行動's actionLogEntries/position,
// translations merged into the チャット transcript by `seq`).
import { useCallback, useEffect, useRef, useState } from "preact/hooks";
import { NpcSocket, type ConnectionState } from "../lib/ws";
import { getChatHistory, type ChatHistoryEntry } from "../lib/api";
import type {
  AvatarRef,
  CharacterRef,
  ClientMessage,
  DriveState,
  PersonRecord,
  ServerMessage,
} from "../lib/types";
import { IDLE_SPEAKING_LEVEL, type SpeakingLevelReading } from "../vrm/level";

const MAX_ENTRIES = 500;
// Sparkline history for the 感情 tab: ~60 turns is plenty for a trend line
// without holding onto unbounded state.
const MAX_AFFECT_HISTORY = 60;

export type TimelineEntry =
  | { id: number; kind: "chat"; role: "user" | "assistant"; text: string; ts: number }
  | { id: number; kind: "sense"; senseKind: "vision" | "speech"; text: string; ts: number }
  | { id: number; kind: "memory"; memoryKind: "short" | "long"; text: string; ts: number }
  | { id: number; kind: "actionLog"; text: string; ts: number }
  /** A turn the NPC deliberately left unanswered — nothing was said, so
   *  there is no text, only why it stayed quiet. */
  | { id: number; kind: "silent"; reason: string; ts: number };

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
  /** Monotonic arrival sequence, drawn from the same `seqRef` counter as
   *  `TimelineEntry` ids, so the チャット transcript can merge translation
   *  rows into its id-ordered row list. Assigned once when the entry is
   *  first created and never reassigned by later per-language frames, so a
   *  translation keeps the position it first appeared at. */
  seq: number;
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
  /** Conversation partner these readings belong to — see AffectMessage. */
  partner: string | null;
  partnerKnown: boolean;
  partnerSwitched: boolean;
  partnerAway: boolean;
  drives: DriveState[];
}

export interface UseNpcSocketResult {
  connectionState: ConnectionState;
  version: string | null;
  modules: Record<string, boolean>;
  character: CharacterRef | null;
  /** The VRM to display, resolved server-side from the active character's
   *  own avatar or the standalone default — null for neither. Independent of
   *  `character`, so an avatar works with no character sheet loaded. */
  avatar: AvatarRef | null;
  timeline: TimelineEntry[];
  ttsLines: TtsLineEntry[];
  translations: TranslationEntry[];
  visionLog: SenseEntry[];
  actionLogEntries: ActionLogEntry[];
  position: PositionState | null;
  volume: number;
  errors: ErrorEntry[];
  /** Whether the cascade voice loop (mic -> STT -> talk -> TTS) is running,
   *  as last reported by the server's `voice` frame. Server-authoritative:
   *  the チャット tab's 開始/停止 toggle sends `voiceStart`/`voiceStop` and
   *  waits for the echoed frame rather than flipping optimistically, so two
   *  open tabs can't disagree about the switch position. */
  voiceActive: boolean;
  /** True while the NPC's synthesized voice is actually audible on the
   *  server's speakers — the VRM avatar's lip-sync signal. Server-reported
   *  (see `SpeakingMessage`); the browser has no way to derive it, since it
   *  never receives the audio. */
  speaking: boolean;
  /**
   * Loudness of the voice currently audible, for the avatar's lip-sync — see
   * `SpeakingLevelMessage`.
   *
   * A ref, not state, and the only value here that is: readings arrive ~20
   * times a second, and the sole consumer is a render loop that reads it
   * directly (see components/VrmStage.tsx). Putting it in state would
   * re-render the whole app at that rate to deliver a number React itself
   * never displays. Reset to a zero level whenever `speaking` goes false or
   * the socket drops, so a stale reading can't leave the mouth part-open on
   * a line that already ended.
   */
  speakingLevelRef: { readonly current: SpeakingLevelReading };
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
  /**
   * Whether the last `suspend`/`resume` this client sent was confirmed
   * applied by the server -- null until either ack has arrived at least
   * once. This is the *only* feedback the 音声 panel's Pause/Resume buttons
   * get at all (see VoicePanel.tsx); `voiceActive` doesn't stand in for it,
   * since that tracks the unrelated master mic-to-speaker loop switch.
   *
   * Known to go stale in one specific way: npc-speech auto-resumes a
   * suspended queue on its own after a timeout, and that isn't a reply to a
   * client `resume` frame, so it isn't guaranteed to arrive with a matching
   * `resumeAccepted` -- this can therefore still read `true` for a while
   * after the server has already resumed by itself. Treated as an honest
   * limitation rather than something to work around: the alternative would
   * be inventing a polling or timeout mechanism on the client to guess when
   * the server's own timeout fires, which is worse than admitting the
   * signal is "last confirmed action", not "current state".
   */
  ttsSuspended: boolean | null;
  send: (msg: ClientMessage) => void;
}

function cap<T>(list: T[], max: number): T[] {
  return list.length > max ? list.slice(list.length - max) : list;
}

/**
 * Reduces a `suspendAccepted`/`resumeAccepted` ack into `ttsSuspended`'s next
 * value; any other frame type is a no-op. Pulled out of `handleMessage`'s
 * switch as its own plain function purely so it's something vitest can
 * exercise directly, without standing up the whole hook (see
 * useNpcSocket.test.ts) -- the logic itself is a two-line mapping, not
 * something that needed splitting out for its own sake.
 */
export function nextTtsSuspended(current: boolean | null, msg: ServerMessage): boolean | null {
  if (msg.type === "suspendAccepted") return true;
  if (msg.type === "resumeAccepted") return false;
  return current;
}

/** One chat-log turn reduced to the fields a timeline row needs, with `ts`
 *  already resolved from the log's RFC3339 `time` string. Kept separate from
 *  `TimelineEntry` because the row's `id` isn't known yet at this point --
 *  the two call sites that produce these (the mount-time backfill and the
 *  reconnect gap-fill, both below) assign ids very differently; see
 *  `loadHistory` and `fillGap` inside the hook. */
interface DraftChatRow {
  role: "user" | "assistant";
  text: string;
  ts: number;
}

/** Flattens chat-log entries into draft rows, oldest first -- one entry can
 *  contribute a user row, an assistant row, both, or (for a turn the NPC
 *  left silent) neither, mirroring how `chat_log_payload` on the Rust side
 *  writes an empty `output` rather than omitting the entry. */
function draftRowsFromHistory(history: ChatHistoryEntry[]): DraftChatRow[] {
  const rows: DraftChatRow[] = [];
  for (const entry of history) {
    const parsed = Date.parse(entry.time);
    const ts = Number.isNaN(parsed) ? Date.now() : parsed;
    if (entry.input) rows.push({ role: "user", text: entry.input, ts });
    if (entry.output) rows.push({ role: "assistant", text: entry.output, ts });
  }
  return rows;
}

// How many of the *not-otherwise-accounted-for* rows at the trailing edge of
// a history fetch get checked for a live-frame collision -- see
// `dedupeAgainstTimeline`. Bounded by how long the GET itself takes to
// round-trip, not by how long a connection was down for, so the same small
// number covers both the mount-time backfill and a reconnect gap-fill.
const RACE_WINDOW = 8;

/**
 * Filters freshly-fetched chat-log rows down to the ones that aren't already
 * visible in `existing`, guarding two different duplicate risks at two
 * different scopes:
 *
 * 1. A row already merged in by an *earlier* history fetch (the mount-time
 *    backfill, or a previous reconnect's gap-fill) is dropped no matter how
 *    far back it sits in this fetch -- safe without a size bound because the
 *    match key includes `ts`, and a history row's `ts` is
 *    `Date.parse(entry.time)`: the exact same log line, refetched later,
 *    parses to the exact same number every time (chat-log.jsonl is
 *    append-only and rotation keeps surviving lines verbatim, so `time`
 *    never changes under an entry once written). Two genuinely different
 *    turns that happen to share text (the user really can say "うん" twice,
 *    minutes apart) essentially never share a millisecond-precision
 *    timestamp too, so this can't eat a legitimate repeat.
 *
 * 2. A row that duplicates a *live* frame racing this same fetch --
 *    chat-log.jsonl is written by a separate bus subscriber (see
 *    npc-memory) than the one that broadcasts the live `chat` frame, so
 *    there's no ordering guarantee between "the frame reached this client"
 *    and "the line reached the log". That race is bounded by how long the
 *    GET itself takes, not by how long the connection was down for, so it
 *    can only ever touch the trailing `RACE_WINDOW` rows of whatever's left
 *    after step 1 -- everything earlier has had no opportunity to collide
 *    with anything live. This check can't use `ts` (a live frame's `ts` is
 *    the server's broadcast clock, not chat-log's write time, and the two
 *    don't agree closely enough to compare) so it matches on role+text
 *    alone, restricted to that trailing window for the same "don't eat a
 *    legitimate repeat" reason as step 1.
 */
function dedupeAgainstTimeline(rows: DraftChatRow[], existing: TimelineEntry[]): DraftChatRow[] {
  const exactKeys = new Set(existing.flatMap((e) => (e.kind === "chat" ? [`${e.role}:${e.text}:${e.ts}`] : [])));
  const notAlreadyMerged = rows.filter((row) => !exactKeys.has(`${row.role}:${row.text}:${row.ts}`));

  const liveChatKeys = new Set(existing.flatMap((e) => (e.kind === "chat" ? [`${e.role}:${e.text}`] : [])));
  const tailStart = notAlreadyMerged.length - RACE_WINDOW;
  return notAlreadyMerged.filter((row, i) => i < tailStart || !liveChatKeys.has(`${row.role}:${row.text}`));
}

export function useNpcSocket(): UseNpcSocketResult {
  const [connectionState, setConnectionState] = useState<ConnectionState>("connecting");
  const [version, setVersion] = useState<string | null>(null);
  const [modules, setModules] = useState<Record<string, boolean>>({});
  const [character, setCharacter] = useState<CharacterRef | null>(null);
  const [avatar, setAvatar] = useState<AvatarRef | null>(null);
  const [timeline, setTimeline] = useState<TimelineEntry[]>([]);
  const [ttsLines, setTtsLines] = useState<TtsLineEntry[]>([]);
  const [translations, setTranslations] = useState<TranslationEntry[]>([]);
  const [visionLog, setVisionLog] = useState<SenseEntry[]>([]);
  const [actionLogEntries, setActionLogEntries] = useState<ActionLogEntry[]>([]);
  const [position, setPosition] = useState<PositionState | null>(null);
  const [volume, setVolume] = useState(0);
  const [errors, setErrors] = useState<ErrorEntry[]>([]);
  const [voiceActive, setVoiceActive] = useState(true);
  const [speaking, setSpeaking] = useState(false);
  const [pending, setPending] = useState(false);
  const [affect, setAffect] = useState<AffectSnapshot | null>(null);
  const [affectHistory, setAffectHistory] = useState<AffectSnapshot[]>([]);
  const [memoryVersion, setMemoryVersion] = useState(0);
  const [people, setPeople] = useState<PersonRecord[]>([]);
  const [peopleVersion, setPeopleVersion] = useState(0);
  const [ttsSuspended, setTtsSuspended] = useState<boolean | null>(null);

  const socketRef = useRef<NpcSocket | null>(null);
  const seqRef = useRef(0);
  const speakingLevelRef = useRef<SpeakingLevelReading>(IDLE_SPEAKING_LEVEL);

  /** Record a loudness reading, bumping `seq` so the animator can tell a
   *  repeated value apart from a stalled feed (see vrm/level.ts). */
  const pushSpeakingLevel = useCallback((level: number) => {
    speakingLevelRef.current = { level, seq: speakingLevelRef.current.seq + 1 };
  }, []);

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
    // than overwriting) is what avoids losing any such frames; deduping
    // against `prev` (see `dedupeAgainstTimeline`) is what stops one of them
    // from being shown twice when the same turn also comes back in
    // `history` (that function's doc comment covers the race in detail).
    async function loadHistory() {
      try {
        const { entries: history } = await getChatHistory(200);
        if (cancelled || history.length === 0) return;
        const drafts = draftRowsFromHistory(history);
        setTimeline((prev) => {
          const survivors = dedupeAgainstTimeline(drafts, prev);
          if (survivors.length === 0) return prev;
          const base = -survivors.length;
          const rows: TimelineEntry[] = survivors.map((d, i) => ({
            id: base + i,
            kind: "chat",
            role: d.role,
            text: d.text,
            ts: d.ts,
          }));
          return cap([...rows, ...prev], MAX_ENTRIES);
        });
      } catch (err) {
        console.error("failed to load chat history", err);
      }
    }
    void loadHistory();

    // True once the socket has reached `open` at least once. A *later* open
    // is a reconnect: unlike the first connect, whatever frames the server
    // sent while the connection was down were never received at all (see
    // lib/ws.ts -- it only retries the connection, it can't replay what the
    // old one missed), so `fillGap` below refetches history to recover them.
    // The first open doesn't need this: `loadHistory` above already covers
    // it, unconditionally, on mount.
    let everOpened = false;
    // Set while a gap-fill fetch is in flight, so a connection that flaps
    // open/closed/open again before the first fetch resolves doesn't kick
    // off a second one racing it.
    let gapFillInFlight = false;

    /**
     * Refetches chat history after a reconnect and merges in whatever it
     * finds that wasn't already on screen -- the turns lost while the
     * connection was down. Failure is logged and otherwise a no-op, leaving
     * the transcript exactly as it was, same as `loadHistory`'s failure
     * mode.
     *
     * Id scheme: `base` is the id counter's value at the moment this
     * reconnect was detected -- the id of the last row already on screen
     * before the drop (or 0, if none yet). It's captured, and the counter
     * bumped past it, *synchronously* here, before the `await` below yields
     * -- so no live frame arriving while this fetch is in flight can be
     * assigned `base + 1` out from under it; nextId() will only ever hand
     * out `base + 2` onward for as long as this fetch is pending. Every
     * surviving row is then given a fractional id strictly between `base`
     * and `base + 1` (`base + (i+1)/(n+1)`, which stays inside that open
     * interval no matter how large `n` is), preserving their relative
     * order. Once ChatView sorts by id, that places them exactly where they
     * belong: after everything shown before the drop, and before any live
     * frame that arrived once the connection came back -- without reusing
     * or renumbering either range. A second reconnect before any live frame
     * arrives still gets a fresh, higher `base` (the bump above already
     * moved the counter past the first gap-fill's interval), so back-to-back
     * gap-fills can't collide with each other either.
     */
    async function fillGap() {
      if (cancelled || gapFillInFlight) return;
      gapFillInFlight = true;
      const base = seqRef.current;
      seqRef.current += 1;
      try {
        const { entries: history } = await getChatHistory(200);
        if (cancelled || history.length === 0) return;
        const drafts = draftRowsFromHistory(history);
        setTimeline((prev) => {
          const survivors = dedupeAgainstTimeline(drafts, prev);
          if (survivors.length === 0) return prev;
          const rows: TimelineEntry[] = survivors.map((d, i) => ({
            id: base + (i + 1) / (survivors.length + 1),
            kind: "chat",
            role: d.role,
            text: d.text,
            ts: d.ts,
          }));
          // `prev` is always kept sorted by id (history is spliced in with
          // ids that fit its existing gap, live handlers only ever append
          // with an ever-increasing nextId()), so splicing these in right
          // before the first row whose id exceeds `base` keeps that
          // invariant intact -- which matters for `cap` below: it trims
          // from the *front* of the array, and that's only "the oldest
          // rows" if array order and id order agree.
          const insertAt = prev.findIndex((e) => e.id > base);
          const at = insertAt === -1 ? prev.length : insertAt;
          return cap([...prev.slice(0, at), ...rows, ...prev.slice(at)], MAX_ENTRIES);
        });
      } catch (err) {
        console.error("failed to refill chat history after reconnect", err);
      } finally {
        gapFillInFlight = false;
      }
    }

    function handleMessage(msg: ServerMessage) {
      const now = Date.now();
      switch (msg.type) {
        case "hello":
          setVersion(msg.version);
          setModules(msg.modules);
          setCharacter(msg.character);
          // Normalized to null so consumers have one "no avatar" value
          // rather than having to check for both undefined and null.
          setAvatar(msg.avatar ?? null);
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

        case "silent":
          // Stands in for the assistant `chat` frame this turn didn't
          // produce: it ends the turn (typing indicator off) and leaves a
          // muted line so an unanswered message doesn't look like a hang.
          setTimeline((prev) =>
            cap([...prev, { id: nextId(), kind: "silent", reason: msg.reason, ts: msg.ts }], MAX_ENTRIES),
          );
          setPending(false);
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
            // Normalized to null so consumers have one "nobody named" value
            // rather than having to check for both undefined and "".
            partner: msg.partner ?? null,
            partnerKnown: msg.partnerKnown,
            partnerSwitched: msg.partnerSwitched,
            partnerAway: msg.partnerAway,
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
                    seq: nextId(),
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

        case "voice":
          setVoiceActive(msg.active);
          break;

        case "speaking":
          setSpeaking(msg.active);
          // A stopped clip means no more `speakingLevel` frames are coming
          // until the next one starts — clear the reading now rather than
          // leaving whatever level the clip ended on sitting there stale.
          if (!msg.active) pushSpeakingLevel(0);
          break;

        case "speakingLevel":
          pushSpeakingLevel(msg.level);
          break;

        case "avatar":
          // Pushed whenever the answer changes, so choosing a model in the
          // キャラ tab lights up the チャット tab's avatar layout right away
          // — in this tab and every other open one.
          setAvatar(msg.avatar);
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

        case "suspendAccepted":
        case "resumeAccepted":
          // The one ack pair genuinely worth tracking here -- see
          // `ttsSuspended`'s doc comment above for why, and `nextTtsSuspended`
          // for the (trivial, independently-tested) mapping.
          setTtsSuspended((prev) => nextTtsSuspended(prev, msg));
          break;

        case "eventAccepted":
        case "interruptAccepted":
          // Acks aimed at out-of-process extension clients, not this UI --
          // see EventAcceptedMessage/InterruptAcceptedMessage's doc comments
          // in lib/types.ts for why each has nothing for a human-facing tab
          // to react to. Listed explicitly (rather than falling through to
          // `default`) so a reviewer scanning this switch can see they were
          // considered and deliberately skipped, not simply forgotten.
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
        // coming; don't leave the typing indicator stuck forever — nor the
        // avatar's mouth, which would otherwise keep flapping until the
        // connection came back and the next clip ended.
        if (state !== "open") {
          setPending(false);
          setSpeaking(false);
          pushSpeakingLevel(0);
          return;
        }
        // See `everOpened`/`fillGap` above: only a *reconnect* (not the
        // first successful open) needs to refetch history, since only a
        // reconnect could have a gap to fill.
        if (everOpened) {
          void fillGap();
        } else {
          everOpened = true;
        }
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
    avatar,
    timeline,
    ttsLines,
    translations,
    visionLog,
    actionLogEntries,
    position,
    volume,
    errors,
    voiceActive,
    speaking,
    speakingLevelRef,
    pending,
    affect,
    affectHistory,
    memoryVersion,
    people,
    peopleVersion,
    ttsSuspended,
    send,
  };
}
