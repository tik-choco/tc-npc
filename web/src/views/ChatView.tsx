// チャット tab: a two-column workspace. The left column (.chat-main) is the
// merged transcript (user/assistant bubbles, memory/action/vision/error
// entries rendered as muted system lines) + composer + interrupt. Speech
// heard through the mic is a user bubble here, identical to a typed one
// apart from a small ear glyph on the turn's name row — see `speechAsChat`.
// Since translations were pulled out of the 通訳 sidebar tab, each
// translation entry attaches its per-language rows directly beneath the row
// that already shows its original text — the bubble above it already
// displays the original once, so it isn't repeated. A translation
// only falls back to its own standalone dashed bubble when no such source
// row can be found (see `attachTranslations` below). The right column is a
// 320px sidebar (ChatSidebar) that carries 音声 / 状態 / 通訳: it pill-tabs
// between a 音声 panel (volume meter, suspend/resume, TTS log), a 状態 panel
// (a condensed 内心 affect readout plus connection/version/character/
// modules), and a 通訳 panel that is
// settings-only now (the live translation log it used to show lives here in
// the transcript instead), mirroring agent-speech's transcript-plus-sidebar
// shape.
//
// The transcript renders turns rather than bare bubbles: consecutive
// same-speaker messages group under one avatar/name with a single trailing
// timestamp (see `describeRows`), days are separated, and the live edge can
// be rejoined from the floating jump button once the reader has scrolled
// away from it.
//
// The header's 音声開始/停止 toggle is the cascade voice loop's master switch
// (mic -> VAD/STT -> talk -> TTS, all server-side in npc-speech). It sends
// `voiceStart`/`voiceStop` and renders `voiceActive` back from the server
// rather than its own optimistic state, so several open tabs can't disagree
// about the switch position — while it's on, the user can simply talk to the
// NPC instead of typing, the same way agent-speech works.
import { Fragment } from "preact";
import { useEffect, useMemo, useRef, useState } from "preact/hooks";
import {
  Activity,
  AlertTriangle,
  ArrowDown,
  Brain,
  Check,
  Copy,
  Ear,
  Eye,
  Languages,
  MessageSquare,
  MessageSquareOff,
  Mic,
  MicOff,
  OctagonX,
  SendHorizontal,
  UserRound,
} from "lucide-preact";
import type {
  AffectSnapshot,
  ErrorEntry,
  TimelineEntry,
  TranslationEntry,
  TtsLineEntry,
} from "../hooks/useNpcSocket";
import { useAutoScroll } from "../hooks/useAutoScroll";
import type { ConnectionState } from "../lib/ws";
import type { ChatPanel } from "../lib/router";
import { ConnectionStatus } from "../components/ConnectionStatus";
import { ChatSidebar } from "../components/ChatSidebar";
import { useI18n } from "../hooks/useI18n";
import type { Lang, Translate } from "../lib/i18n";
import "../styles/components.css";
import "../styles/chat.css";

/** An `ErrorEntry` reshaped to slot into the transcript alongside
 *  `TimelineEntry`, ordered by the same monotonic id sequence. */
type ErrorRow = { id: number; kind: "error"; text: string; ts: number };
/** A `TranslationEntry` reshaped the same way. `id` is the entry's `seq`
 *  (not its own string `id`, which is a server-assigned upsert key used to
 *  fold later per-language frames into the same entry, not an ordering
 *  value) — `seq` is drawn from the same monotonic counter as
 *  `TimelineEntry.id`/`ErrorEntry.id`, so sorting all three by `id` below
 *  reproduces arrival order. */
type TranslationRow = { id: number; kind: "translation"; entry: TranslationEntry };
/** A chat row, plus the marker `speechAsChat` sets on a turn that arrived
 *  through the mic rather than the composer. */
type ChatRow = Extract<TimelineEntry, { kind: "chat" }> & { spoken?: boolean };
type Row = ChatRow | Exclude<TimelineEntry, { kind: "chat" }> | ErrorRow | TranslationRow;

/**
 * Recasts a heard-speech `sense` row as the user turn it actually is.
 *
 * Speech reaches the UI on its own frame (`sense`/`speech`, see
 * crates/npc-server/src/bus_forward.rs) rather than as a `chat` row, because
 * the server only mints a `chat` frame for input it received over the socket
 * — but from the transcript's point of view a sentence the NPC heard is the
 * same turn as one that was typed, and used to be drawn as a muted system
 * line mirrored to the right edge to half-suggest that. Now it just becomes a
 * user bubble; only the small ear glyph on the turn's name row says it was
 * spoken. Nothing is duplicated by this: `EchoGuard` already suppresses the
 * `sense` frame for speech the socket itself sent (typing), so exactly one of
 * the two shapes exists per turn.
 *
 * History rows backfilled from chat-log.jsonl have no such marker — the log
 * records what was said, not which input device it came from — so a reloaded
 * transcript shows those turns as plain user bubbles.
 */
function speechAsChat(entry: TimelineEntry): TimelineEntry | ChatRow {
  if (entry.kind !== "sense" || entry.senseKind !== "speech") return entry;
  return { id: entry.id, kind: "chat", role: "user", text: entry.text, ts: entry.ts, spoken: true };
}

// How far back `attachTranslations` is willing to scan for a translation's
// source row. The source frame (a `sense`/`chat` row) reaches the UI only a
// few frames before the paired translation entry it seeds — see
// crates/npc-translate: `translate()` republishes the same bus payload text
// verbatim as `original`, and the `lang: ""` frame that creates the entry
// follows immediately after — but other frames (memory, actionLog) can slip
// in between the two, so the window has to be more than 1. It's still kept
// small so a translation can't reach past its real source and latch onto a
// coincidentally-identical line much earlier in the transcript — in
// particular the negative-id rows backfilled from chat history on mount
// (see useNpcSocket.ts), which no live translation should ever attach to.
const ATTACH_WINDOW = 6;

// Consecutive messages from the same speaker are drawn as one group (single
// avatar, single trailing timestamp). A pause longer than this splits them
// again, so a reply hours later doesn't silently merge into the message it
// answers.
const GROUP_GAP_MS = 5 * 60 * 1000;

// How tall the composer is allowed to grow before it starts scrolling
// internally, in px — roughly six lines at the composer's font size.
const COMPOSER_MAX_HEIGHT = 168;

/**
 * Pairs each translation entry with the transcript row that already shows
 * its original text, so the entry's language rows can render attached
 * beneath that row instead of the transcript showing the original twice
 * (once in the row, once in a standalone translation bubble). Returns the
 * source-row-id -> entry map, plus the entries that found no match (kept as
 * standalone `TranslationRow`s — the pre-merge fallback).
 *
 * `base` and `translations` are both already in arrival order (sorted by id
 * / seq from the same monotonic counter), so a single forward pointer over
 * `base` is enough to locate "the last row before this translation's seq"
 * for every entry in one pass.
 */
function attachTranslations(
  base: Exclude<Row, TranslationRow>[],
  translations: TranslationEntry[],
): { attachments: Map<number, TranslationEntry>; orphans: TranslationRow[] } {
  const attachments = new Map<number, TranslationEntry>();
  const claimed = new Set<number>();
  const orphans: TranslationRow[] = [];

  const sorted = [...translations].sort((a, b) => a.seq - b.seq);
  let idx = 0;
  for (const entry of sorted) {
    while (idx < base.length && base[idx].id < entry.seq) idx++;
    const original = entry.original.trim();

    let matchId: number | undefined;
    for (let i = idx - 1, steps = 0; i >= 0 && steps < ATTACH_WINDOW; i--, steps++) {
      const row = base[i];
      // A row must never take two translations, so a claimed row is skipped
      // (not un-skipped later) but still counts against the scan window.
      if (claimed.has(row.id)) continue;
      // Heard speech is a user-role chat row by the time it gets here (see
      // `speechAsChat`), so one check covers both typed and spoken turns.
      const fits =
        row.kind === "chat" && (entry.source === "agent" ? row.role === "assistant" : row.role === "user");
      if (fits && row.text.trim() === original) {
        matchId = row.id;
        break;
      }
    }

    if (matchId !== undefined) {
      attachments.set(matchId, entry);
      claimed.add(matchId);
    } else {
      orphans.push({ id: entry.seq, kind: "translation", entry });
    }
  }
  return { attachments, orphans };
}

function formatTime(ts: number): string {
  if (!ts) return "";
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}`;
}

function isSameDay(a: number, b: number): boolean {
  const x = new Date(a);
  const y = new Date(b);
  return x.getFullYear() === y.getFullYear() && x.getMonth() === y.getMonth() && x.getDate() === y.getDate();
}

/** Day-separator label: 今日/昨日 for the two days a live operator console
 *  actually spends most of its time in, and a locale-formatted date for
 *  anything older (which, given the history backfill, is common enough to be
 *  worth spelling out rather than showing a bare timestamp). */
function formatDay(ts: number, lang: Lang, t: Translate): string {
  const now = Date.now();
  if (isSameDay(ts, now)) return t("chat.day.today");
  if (isSameDay(ts, now - 24 * 60 * 60 * 1000)) return t("chat.day.yesterday");
  return new Intl.DateTimeFormat(lang, { month: "long", day: "numeric", weekday: "short" }).format(new Date(ts));
}

/** The wall-clock time a row should be filed under. Every row kind carries
 *  one, but an orphan `TranslationRow` keeps it on the entry it wraps rather
 *  than on the row itself. */
function rowTs(row: Row): number {
  return row.kind === "translation" ? row.entry.ts : row.ts;
}

/** Per-row render metadata derived in one pass over the merged rows:
 *  where the day changes, and where same-speaker runs start and end. */
interface RowMeta {
  row: Row;
  /** Set on the first row of each calendar day — the separator's label. */
  dayLabel: string | null;
  /** First row of a same-speaker run (renders the avatar + name). */
  head: boolean;
  /** Last row of a same-speaker run (renders the timestamp + bubble notch). */
  tail: boolean;
}

/**
 * Annotates `rows` with grouping and day boundaries. A run is broken by a
 * different speaker, any non-chat row in between, a day change, a pause
 * longer than `GROUP_GAP_MS`, or an attached translation — an attachment
 * renders *below* its bubble, so letting the run continue past it would
 * bury the translation between two bubbles that read as one block.
 */
function describeRows(rows: Row[], attachments: Map<number, TranslationEntry>, lang: Lang, t: Translate): RowMeta[] {
  const groupable = (row: Row): row is ChatRow => row.kind === "chat";

  const continues = (prev: Row | undefined, row: Row | undefined): boolean => {
    if (!prev || !row || !groupable(prev) || !groupable(row)) return false;
    if (prev.role !== row.role) return false;
    // A spoken turn and a typed one are both "you", but only the run's head
    // carries the ear glyph — grouping them would file one under the other's
    // marker. Switching input mid-conversation starts a new run instead.
    if (Boolean(prev.spoken) !== Boolean(row.spoken)) return false;
    if (attachments.has(prev.id)) return false;
    if (prev.ts && row.ts && !isSameDay(prev.ts, row.ts)) return false;
    return !prev.ts || !row.ts || row.ts - prev.ts <= GROUP_GAP_MS;
  };

  let lastDayTs: number | null = null;
  return rows.map((row, i) => {
    // A row with no usable timestamp (history rows whose `time` failed to
    // parse) neither opens a new day nor closes the current one — it just
    // inherits whichever day is already showing.
    const ts = rowTs(row);
    let dayLabel: string | null = null;
    if (ts && (lastDayTs === null || !isSameDay(lastDayTs, ts))) {
      dayLabel = formatDay(ts, lang, t);
      lastDayTs = ts;
    }
    return {
      row,
      dayLabel,
      head: !continues(rows[i - 1], row),
      tail: !continues(row, rows[i + 1]),
    };
  });
}

/** Avatar glyph for the NPC: the character name's first character, taken by
 *  code point so an emoji or a surrogate-pair name doesn't render as half a
 *  character. */
function initialOf(name: string): string {
  return [...name.trim()][0] ?? "N";
}

/** Memory writes are whole documents — a short-term summary or the entire
 *  consolidated long-term memory — so printing one in full buries the
 *  conversation it is supposed to annotate. Clamped to a couple of lines
 *  with a toggle; only long entries get the toggle at all, and expansion is
 *  per-entry state so re-reading one doesn't unfold the rest. */
function MemoryLine({ entry, t }: { entry: Extract<Row, { kind: "memory" }>; t: Translate }) {
  const [expanded, setExpanded] = useState(false);
  // Cheap proxy for "this will wrap past the clamp": either it is long or it
  // is multi-line. A single short line never gets a toggle it doesn't need.
  const long = entry.text.length > 60 || entry.text.includes("\n");
  return (
    <div class="chat-system-line chat-system-line--memory">
      <Brain size={13} aria-hidden="true" />
      <span class={long && !expanded ? "chat-system-clamp" : undefined}>
        <em class="chat-system-tag">
          {entry.memoryKind === "short" ? t("chat.memory.short") : t("chat.memory.long")}
        </em>
        {entry.text}
      </span>
      {long && (
        <button type="button" class="chat-system-toggle" onClick={() => setExpanded((v) => !v)}>
          {expanded ? t("chat.collapse") : t("chat.expand")}
        </button>
      )}
      <time>{formatTime(entry.ts)}</time>
    </div>
  );
}

// Commentary *about* the conversation — what the NPC saw, what it committed
// to memory, errors, action log — as a muted left-aligned line rather than a
// bubble. Heard speech used to come through here too, mirrored to the right
// edge to hint that it was the user's own turn; it is now rendered as an
// actual user bubble instead (see `speechAsChat`), so this handles only the
// non-turn kinds and never carries an attached translation.
function SystemLine({
  entry,
  t,
}: {
  entry: Exclude<Row, { kind: "chat" } | { kind: "translation" }>;
  t: Translate;
}) {
  return entry.kind === "sense" ? (
    (() => {
      const Icon = entry.senseKind === "vision" ? Eye : Ear;
      return (
        <div class="chat-system-line chat-system-line--sense">
          <Icon size={13} aria-hidden="true" />
          <span>{entry.text}</span>
          <time>{formatTime(entry.ts)}</time>
        </div>
      );
    })()
  ) : entry.kind === "memory" ? (
    <MemoryLine entry={entry} t={t} />
  ) : entry.kind === "silent" ? (
    <div class="chat-system-line chat-system-line--silent">
      <MessageSquareOff size={13} aria-hidden="true" />
      <span>{entry.reason === "closing" ? t("chat.silent.closing") : t("chat.silent.declined")}</span>
      <time>{formatTime(entry.ts)}</time>
    </div>
  ) : entry.kind === "error" ? (
    <div class="chat-system-line chat-system-line--error">
      <AlertTriangle size={13} aria-hidden="true" />
      <span>
        <em class="chat-system-tag">{t("chat.error")}</em>
        {entry.text}
      </span>
      <time>{formatTime(entry.ts)}</time>
    </div>
  ) : (
    <div class="chat-system-line">
      <Activity size={13} aria-hidden="true" />
      <span>{entry.text}</span>
      <time>{formatTime(entry.ts)}</time>
    </div>
  );
}

/** The per-language rows shared by both places a translation can render:
 *  attached beneath its source row, or inside the standalone fallback
 *  bubble. `entry.translations` fills in one language at a time as
 *  `translation` frames land (see useNpcSocket.ts), so this renders
 *  whatever subset has arrived so far and falls back to a "pending" line
 *  before the first one lands. */
function TranslationLines({ entry, t }: { entry: TranslationEntry; t: Translate }) {
  const langs = Object.keys(entry.translations);
  if (langs.length === 0) {
    return <p class="chat-translation-pending">{t("chat.translation.pending")}</p>;
  }
  return (
    <>
      {langs.map((lang) => (
        <div class="chat-translation-row" key={lang}>
          <span
            class={`chat-translation-lang-pill${entry.reversed ? " chat-translation-lang-pill--reversed" : ""}`}
            title={entry.reversed ? t("chat.translation.reversed.tooltip") : undefined}
          >
            {entry.reversed ? "⇄ " : ""}
            {lang}
          </span>
          <span class="chat-translation-text">{entry.translations[lang]}</span>
        </div>
      ))}
    </>
  );
}

/** Renders one 通訳 entry as its own bubble in the transcript, aligned like a
 *  chat bubble (user-sourced → right, agent-sourced → left) but styled as a
 *  distinct dashed "system" bubble (`.chat-bubble--translation`) so it reads
 *  as an annotation on the turn beside it rather than another speaker —
 *  mirrors agent-speech's Transcript.tsx `item.kind === 'translation'`
 *  branch. This is now the fallback path only: `attachTranslations` couldn't
 *  find the transcript row that shows this entry's original text (source
 *  row scrolled out of the match window, was itself claimed by another
 *  entry, etc.), so — unlike the attached case — the original is worth
 *  showing here, since no other row on screen is showing it. */
function TranslationBubble({ entry, t }: { entry: TranslationEntry; t: Translate }) {
  const isUser = entry.source === "user";
  return (
    <div class={`chat-turn chat-turn--${isUser ? "user" : "assistant"} chat-turn--head chat-turn--tail`}>
      {/* Empty gutter, not an avatar: this bubble is an annotation rather
          than a speaker, but it still has to line up with the assistant
          bubbles it sits among. */}
      {!isUser && <div class="chat-turn-gutter" aria-hidden="true" />}
      <div class="chat-turn-body">
        <div class="chat-bubble chat-bubble--translation">
          <div class="chat-translation-head">
            <Languages size={13} aria-hidden="true" />
            <span>{t("chat.translation.label")}</span>
            <span class="chat-translation-source-badge">
              {isUser ? t("chat.translation.source.user") : t("chat.translation.source.agent")}
            </span>
          </div>
          <p class="chat-translation-original">{entry.original}</p>
          <TranslationLines entry={entry} t={t} />
        </div>
        <time class="chat-turn-time">{formatTime(entry.ts)}</time>
      </div>
    </div>
  );
}

/** Hover/focus-revealed copy action hanging off a bubble's outer edge.
 *  Absolutely positioned so revealing it never reflows the transcript.
 *  Renders nothing where the Clipboard API is unavailable — notably a plain
 *  `http://<lan-ip>` origin, which this console is routinely opened on from
 *  another machine, and where the button could only ever fail. */
function CopyButton({ text, t }: { text: string; t: Translate }) {
  const [copied, setCopied] = useState(false);
  const timer = useRef<number | undefined>(undefined);

  useEffect(() => () => window.clearTimeout(timer.current), []);

  if (typeof navigator === "undefined" || !navigator.clipboard) return null;

  const label = copied ? t("chat.copied") : t("chat.copy");
  return (
    <button
      type="button"
      class="chat-copy"
      title={label}
      aria-label={label}
      onClick={() => {
        navigator.clipboard.writeText(text).then(
          () => {
            setCopied(true);
            window.clearTimeout(timer.current);
            timer.current = window.setTimeout(() => setCopied(false), 1400);
          },
          () => {
            // Permission denied / document not focused — leave the button in
            // its resting state rather than claiming a copy that didn't
            // happen.
          },
        );
      }}
    >
      {copied ? <Check size={13} /> : <Copy size={13} />}
    </button>
  );
}

/** One chat message, drawn as part of a same-speaker run: `head` carries the
 *  avatar and speaker name, `tail` carries the timestamp and the bubble's
 *  notched corner, and the rows in between are bare bubbles tucked under the
 *  same avatar gutter. */
function ChatTurn({
  row,
  head,
  tail,
  attached,
  speaker,
  avatar,
  t,
}: {
  row: ChatRow;
  head: boolean;
  tail: boolean;
  attached?: TranslationEntry;
  speaker: string;
  avatar: string;
  t: Translate;
}) {
  const isUser = row.role === "user";
  return (
    <div
      class={`chat-turn chat-turn--${row.role}${head ? " chat-turn--head" : ""}${tail ? " chat-turn--tail" : ""}`}
    >
      {!isUser && (
        <div class="chat-turn-gutter" aria-hidden="true">
          {head && <span class="chat-avatar">{avatar}</span>}
        </div>
      )}
      <div class="chat-turn-body">
        {head && (
          <div class="chat-turn-name">
            {isUser ? t("chat.speaker.you") : speaker}
            {/* The only thing left distinguishing a spoken turn from a typed
                one — the bubble itself is deliberately identical. */}
            {row.spoken && <Ear size={12} aria-label={t("chat.speaker.heard")} />}
          </div>
        )}
        <div class="chat-turn-bubble">
          <div class={`chat-bubble chat-bubble--${row.role}`}>
            <div class="chat-bubble-text">{row.text}</div>
          </div>
          <CopyButton text={row.text} t={t} />
        </div>
        {attached && (
          <div class="chat-translation-attached">
            <TranslationLines entry={attached} t={t} />
          </div>
        )}
        {tail && <time class="chat-turn-time">{formatTime(row.ts)}</time>}
      </div>
    </div>
  );
}

function TypingIndicator({ avatar, t }: { avatar: string; t: Translate }) {
  return (
    <div class="chat-turn chat-turn--assistant chat-turn--head chat-turn--tail">
      <div class="chat-turn-gutter" aria-hidden="true">
        <span class="chat-avatar">{avatar}</span>
      </div>
      <div class="chat-turn-body">
        <div class="chat-typing" role="status" aria-label={t("chat.typing")}>
          <span class="chat-typing-dot" />
          <span class="chat-typing-dot" />
          <span class="chat-typing-dot" />
        </div>
      </div>
    </div>
  );
}

/** The cascade voice loop's 開始/停止 switch. `volume` drives a halo around
 *  the mic glyph while the loop is running, which is the only continuous
 *  proof from the chat column that the far-end mic is actually being heard —
 *  the numeric meter lives in the sidebar's 音声 panel. */
function VoiceToggle({
  active,
  volume,
  available,
  connected,
  onStart,
  onStop,
  t,
}: {
  active: boolean;
  volume: number;
  available: boolean;
  connected: boolean;
  onStart: () => void;
  onStop: () => void;
  t: Translate;
}) {
  const level = Math.max(0, Math.min(1, volume));
  const disabled = !available || !connected;
  return (
    <button
      type="button"
      class={`chat-voice${active ? " chat-voice--on" : ""}`}
      style={`--voice-level:${active ? level : 0}`}
      disabled={disabled}
      title={available ? t("chat.voice.hint") : t("chat.voice.disabled")}
      onClick={() => (active ? onStop() : onStart())}
    >
      <span class="chat-voice-glyph">{active ? <MicOff size={15} /> : <Mic size={15} />}</span>
      <span class="chat-voice-label">{active ? t("chat.voice.stop") : t("chat.voice.start")}</span>
    </button>
  );
}

export interface ChatViewProps {
  entries: TimelineEntry[];
  errors: ErrorEntry[];
  pending: boolean;
  connectionState: ConnectionState;
  version: string | null;
  character: { id: string; name: string } | null;
  modules: Record<string, boolean>;
  /** Latest affect frame, shown condensed in the sidebar's 状態 panel so the
   *  NPC's current internal state is readable without leaving the
   *  transcript — the full model stays in the 感情 tab. */
  affect: AffectSnapshot | null;
  volume: number;
  ttsLines: TtsLineEntry[];
  translations: TranslationEntry[];
  /** Server-reported position of the voice loop's 開始/停止 switch. */
  voiceActive: boolean;
  /** Sidebar pill, routed as `#/chat/<panel>` — see lib/router.ts. */
  sidebarPanel: ChatPanel;
  onSidebarPanelChange: (panel: ChatPanel) => void;
  /** `speaker` names who is talking, which npc-talk uses to key the affect
   *  model per conversation partner. Omitted when the field is left blank. */
  onSend: (text: string, speaker?: string) => void;
  onInterrupt: () => void;
  onSuspend: () => void;
  onResume: () => void;
  onVoiceStart: () => void;
  onVoiceStop: () => void;
}

export function ChatView({
  entries,
  errors,
  pending,
  connectionState,
  version,
  character,
  modules,
  affect,
  volume,
  ttsLines,
  translations,
  voiceActive,
  sidebarPanel,
  onSidebarPanelChange,
  onSend,
  onInterrupt,
  onSuspend,
  onResume,
  onVoiceStart,
  onVoiceStop,
}: ChatViewProps) {
  const { lang, t } = useI18n();
  const [draft, setDraft] = useState("");
  // Who the operator is speaking *as*. Sent along as the turn's `speaker`,
  // which is what npc-talk keys the per-partner affect state on — leave it
  // blank and the NPC keeps talking to whoever it already was. External
  // clients (the game) set this field on their own frames; this input is how
  // the same thing is reachable from the control panel.
  const [partnerName, setPartnerName] = useState("");
  const composerRef = useRef<HTMLTextAreaElement | null>(null);
  const connected = connectionState === "open";
  const speaker = character?.name ?? t("chat.translation.source.agent");
  const avatar = initialOf(speaker);

  // Errors and translations share the same monotonic id sequence as timeline
  // entries (see useNpcSocket.ts — errors use it directly, translations via
  // `seq`), so merging and sorting by id keeps everything in arrival order.
  // Translations are additionally matched to their source row here (see
  // `attachTranslations`): a matched entry's language rows render attached
  // to that row instead of appearing as its own `rows` entry, so `rows`
  // itself only carries the *unmatched* translations (as `TranslationRow`
  // orphans) alongside the usual timeline/error rows.
  const { items, attachments, lastId } = useMemo(() => {
    const errorRows: ErrorRow[] = errors.map((e) => ({ id: e.id, kind: "error", text: e.message, ts: e.ts }));
    const base: Exclude<Row, TranslationRow>[] = [...entries.map(speechAsChat), ...errorRows].sort(
      (a, b) => a.id - b.id,
    );
    const { attachments, orphans } = attachTranslations(base, translations);
    const rows: Row[] = [...base, ...orphans].sort((a, b) => a.id - b.id);
    return {
      items: describeRows(rows, attachments, lang, t),
      attachments,
      lastId: rows.length > 0 ? rows[rows.length - 1].id : undefined,
    };
  }, [entries, errors, translations, lang, t]);

  // A translation entry's row id is its `seq`, assigned once when the entry
  // is first created (see useNpcSocket.ts) — a later frame that just fills
  // in one more language on an *existing* entry doesn't touch `seq`, so
  // `lastId` alone wouldn't change and the transcript wouldn't follow a
  // bubble that's visibly growing. Folding in a cheap running count of all
  // filled-in language strings across every translation entry gives the key
  // something that does change on every such fill.
  const translationFillCount = translations.reduce((sum, entry) => sum + Object.keys(entry.translations).length, 0);
  // Keyed on the newest row's id (not row count): the underlying lists are
  // capped (MAX_ENTRIES in useNpcSocket.ts), so once capped the count stops
  // changing forever even as new entries keep replacing old ones. The
  // pending flag is folded in too, so the typing indicator's appearance/
  // disappearance also triggers the near-bottom follow.
  const {
    ref: scrollRef,
    atBottom,
    scrollToBottom,
  } = useAutoScroll(`${lastId ?? "none"}:${pending}:${translationFillCount}`);

  // Grow the composer with its content up to COMPOSER_MAX_HEIGHT. Resetting
  // to `auto` first is what lets it shrink again as text is deleted —
  // `scrollHeight` never reports less than the current fixed height.
  useEffect(() => {
    const el = composerRef.current;
    if (!el) return;
    el.style.height = "auto";
    el.style.height = `${Math.min(el.scrollHeight, COMPOSER_MAX_HEIGHT)}px`;
  }, [draft]);

  function submit() {
    const text = draft.trim();
    if (!text || !connected) return;
    // A blank speaker is sent as undefined, not "": the server treats a blank
    // name as "unknown" anyway, and npc-talk reads "unknown" as "no
    // information about who is speaking", which keeps the current partner
    // rather than reading as a switch away from them.
    onSend(text, partnerName.trim() || undefined);
    setDraft("");
  }

  const placeholder = !connected
    ? t("chat.placeholder.disconnected")
    : voiceActive && modules.speech
      ? t("chat.placeholder.voice")
      : t("chat.placeholder");

  return (
    <div class="chat-view">
      <div class="chat-main">
        <div class="chat-toolbar">
          <div class="chat-toolbar-identity">
            <span class="chat-avatar chat-avatar--lg" aria-hidden="true">
              {avatar}
            </span>
            <div class="chat-toolbar-text">
              <span class="chat-toolbar-name">{speaker}</span>
              <ConnectionStatus state={connectionState} />
            </div>
          </div>
          <div class="chat-toolbar-actions">
            <VoiceToggle
              active={voiceActive}
              volume={volume}
              available={Boolean(modules.speech)}
              connected={connected}
              onStart={onVoiceStart}
              onStop={onVoiceStop}
              t={t}
            />
            <button
              type="button"
              class="btn btn-danger btn-small"
              onClick={onInterrupt}
              disabled={!connected}
            >
              <OctagonX size={14} />
              {t("chat.interrupt")}
            </button>
          </div>
        </div>

        <div class="chat-transcript-wrap">
          <div class="chat-transcript" ref={scrollRef} role="log" aria-live="polite">
            {items.length === 0 && !pending && (
              <div class="empty-state">
                <span class="empty-state-icon">
                  <MessageSquare size={24} />
                </span>
                <div class="empty-state-title">{t("chat.empty.title")}</div>
                <div class="empty-state-description">{t("chat.empty.desc")}</div>
              </div>
            )}
            {items.map(({ row, dayLabel, head, tail }) => (
              <Fragment key={row.id}>
                {dayLabel && (
                  <div class="chat-day">
                    <span>{dayLabel}</span>
                  </div>
                )}
                {row.kind === "chat" ? (
                  <ChatTurn
                    row={row}
                    head={head}
                    tail={tail}
                    attached={attachments.get(row.id)}
                    speaker={speaker}
                    avatar={avatar}
                    t={t}
                  />
                ) : row.kind === "translation" ? (
                  <TranslationBubble entry={row.entry} t={t} />
                ) : (
                  <SystemLine entry={row} t={t} />
                )}
              </Fragment>
            ))}
            {pending && <TypingIndicator avatar={avatar} t={t} />}
          </div>

          {!atBottom && (
            <button type="button" class="chat-jump" onClick={() => scrollToBottom()}>
              <ArrowDown size={14} />
              {t("chat.jump")}
            </button>
          )}
        </div>

        <div class="chat-composer">
          <div class="chat-composer-shell">
            <textarea
              ref={composerRef}
              rows={1}
              placeholder={placeholder}
              value={draft}
              disabled={!connected}
              onInput={(e) => setDraft((e.target as HTMLTextAreaElement).value)}
              onKeyDown={(e) => {
                // Shift+Enter inserts a newline; a bare Enter sends. The
                // isComposing guard keeps an IME's confirm-candidate Enter
                // from firing the send — essential for the JP/ZH locales.
                if (e.key === "Enter" && !e.shiftKey && !e.isComposing) {
                  e.preventDefault();
                  submit();
                }
              }}
            />
            <button
              type="button"
              class="chat-send"
              onClick={submit}
              disabled={!connected || !draft.trim()}
              title={t("common.send")}
              aria-label={t("common.send")}
            >
              <SendHorizontal size={16} />
            </button>
          </div>
          <div class="chat-composer-foot">
            <label class="chat-composer-speaker">
              <UserRound size={13} aria-hidden="true" />
              <input
                type="text"
                value={partnerName}
                disabled={!connected}
                placeholder={t("chat.composer.speaker.placeholder")}
                aria-label={t("chat.composer.speaker.label")}
                onInput={(e) => setPartnerName((e.target as HTMLInputElement).value)}
              />
            </label>
            <div class="chat-composer-hint">{t("chat.composer.hint")}</div>
          </div>
        </div>
      </div>

      <aside class="chat-sidebar">
        <ChatSidebar
          connectionState={connectionState}
          version={version}
          character={character}
          modules={modules}
          affect={affect}
          volume={volume}
          ttsLines={ttsLines}
          voiceActive={voiceActive}
          activePanel={sidebarPanel}
          onPanelChange={onSidebarPanelChange}
          onSuspend={onSuspend}
          onResume={onResume}
        />
      </aside>
    </div>
  );
}
