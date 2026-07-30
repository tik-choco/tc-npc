// WS protocol contract shared with the Rust server (crates/npc-server). Keep
// this in sync with the server side — it is intentionally the single source
// of truth for frame shapes on the web UI.

/** A 3D model, naming a `.vrm` in the server's model folder
 *  (`{data_dir}/vrm/`). Fetched for display over `/api/vrm/file/:file`. */
export interface AvatarRef {
  /** Always `"vrm"` today; tagged so another avatar kind could be added. */
  kind: string;
  file: string;
}

/** A character as referenced by `hello` and `/api/state`. */
export interface CharacterRef {
  id: string;
  name: string;
}

export interface HelloMessage {
  type: "hello";
  version: string;
  modules: Record<string, boolean>;
  character: CharacterRef | null;
  /**
   * The model to display, or absent for none. Deliberately a sibling of
   * `character` rather than a field on it: the NPC holds a conversation with
   * no character sheet loaded, so an avatar must not require one. The server
   * resolves this to the active character's own avatar when there is one and
   * otherwise to the standalone `config.character.avatar_file`, and drops a
   * reference whose file is no longer in the folder.
   */
  avatar?: AvatarRef | null;
}

export interface ChatMessage {
  type: "chat";
  role: "user" | "assistant";
  text: string;
  ts: number;
}

/** The NPC took a turn without saying anything: the conversation is over
 *  (`closing`) or it declined to answer this utterance (`declined`). Sent
 *  instead of the `chat` frame that would normally end the turn — see
 *  crates/npc-talk/src/style.rs. */
export interface SilentMessage {
  type: "silent";
  reason: string;
  ts: number;
}

/**
 * One synthesized sentence, sent per SENTENCE by whichever module actually
 * speaks it (npc-speech, or an external TTS extension) -- not once for the
 * whole reply the way earlier versions of this frame worked. That also means
 * it is correctly *absent* whenever nothing is being read aloud (TTS turned
 * off in config, or a reply so far unspoken): no frame is the expected
 * behaviour there, not a gap to paper over -- see VoicePanel.tsx's empty
 * state for how the log distinguishes "nothing spoken yet" from "TTS is off,
 * so nothing ever will be".
 */
export interface TtsLineMessage {
  type: "ttsLine";
  text: string;
  translations?: Record<string, string>;
}

export interface SenseMessage {
  type: "sense";
  kind: "vision" | "speech";
  text: string;
  ts: number;
}

/**
 * One simultaneous-interpretation update from npc-translate. Every frame for
 * the same utterance shares an `id`: the first arrives with `lang: ""` as
 * soon as the line is heard (translations still pending), then one frame per
 * target language as each translation lands. `reversed` marks an
 * auto-reversed entry (a reply in a target language translated back into the
 * source language).
 */
export interface TranslationMessage {
  type: "translation";
  id: string;
  source: "user" | "agent";
  original: string;
  lang: string;
  text: string;
  reversed: boolean;
  ts: number;
}

export interface MemoryMessage {
  type: "memory";
  kind: "short" | "long";
  text: string;
}

/** One of the 22 neurotransmitter-analogue drives that make up the affect
 *  model. `key` is a fixed snake_case id (`substance_p` is the one with an
 *  underscore); `level`/`base` are both 0..1. */
export interface DriveState {
  key: string;
  level: number;
  base: number;
}

/**
 * Emitted once per conversation turn by the affect engine. `drives` always
 * carries all 22 entries in a fixed order. `closing` marks a wind-down
 * phase of the conversation; `inviteCaution` marks a first-meeting
 * invitation the NPC is wary of.
 *
 * The state is per conversation partner (npc-talk's `PartnerAffect`), so
 * these readings belong to `partner` specifically — familiarity built with
 * one person is not what you see when another is speaking.
 */
export interface AffectMessage {
  type: "affect";
  ts: number;
  familiarity: number;
  closing: boolean;
  inviteCaution: boolean;
  /** Who the NPC is talking to. Omitted while nobody has identified
   *  themselves — messages typed into this UI carry no speaker, so an
   *  operator-only session stays unnamed. */
  partner?: string;
  /** The NPC has talked with this partner before in this session. */
  partnerKnown: boolean;
  /** Set on the single frame where npc-talk detected the partner changing.
   *  Server-side detection, so the UI doesn't have to diff frames. */
  partnerSwitched: boolean;
  /** The partner has been silent past `talk.affect.absence_timeout_secs`, so
   *  the NPC treats them as having left: drives settle back to baseline
   *  while what it knows about them is kept. Unlike `partnerSwitched` this
   *  is a lasting state, cleared only when somebody speaks again. Its frame
   *  arrives on a timer, not on a conversation turn. */
  partnerAway: boolean;
  drives: DriveState[];
}

export interface ActionLogMessage {
  type: "actionLog";
  text: string;
}

export interface PositionMessage {
  type: "position";
  x: number;
  y: number;
  heading: number;
}

export interface VolumeMessage {
  type: "volume";
  level: number;
}

/**
 * The NPC's synthesized voice just became (or stopped being) audible on the
 * server's speakers — published by npc-speech's playback thread on each
 * transition, not per line.
 *
 * This exists because the browser never receives the audio: synthesis and
 * playback both happen host-side, so the only way the VRM avatar's mouth can
 * move in time with the actual voice is for the process driving the speaker
 * to say so. `ttsLine` can't stand in for it — that frame is sent when the
 * *text* is ready, before synthesis has even been requested.
 */
export interface SpeakingMessage {
  type: "speaking";
  active: boolean;
}

/**
 * Loudness of the NPC's voice as it is actually being played, sampled by the
 * same playback thread that emits `SpeakingMessage` — roughly every 50ms
 * while a clip is audible, plus one final `0` when it ends. `level` is
 * 0.0..=1.0.
 *
 * Kept as its own frame rather than a field added to `SpeakingMessage`:
 * `speaking` is a rare on/off transition (one publish per clip start/stop),
 * so every client — including one that predates this frame — can treat it as
 * the authoritative mouth on/off switch, while this one arrives on a fast,
 * continuous timer that only a client actually animating a lip-sync envelope
 * needs to bother subscribing to. A client that ignores it (or a server too
 * old to send it) still gets a correct mouth via the sine-based fallback in
 * vrm/animation.ts, keyed off `speaking` alone.
 */
export interface SpeakingLevelMessage {
  type: "speakingLevel";
  level: number;
}

/**
 * Which model to display has changed — a VRM was assigned or cleared, a
 * model was added to or removed from the folder, or a different character
 * was activated.
 *
 * `hello` carries the avatar only at connect time, so without this frame a
 * tab that was already open when the model was chosen would keep its avatar
 * layout disabled until reloaded. `avatar` is null when there is none.
 */
export interface AvatarMessage {
  type: "avatar";
  avatar: AvatarRef | null;
}

export interface StatusMessage {
  type: "status";
  modules: Record<string, boolean>;
}

/** Current position of the cascade voice loop's 開始/停止 switch. Sent right
 *  after `hello` and again on every change, from whichever tab flipped it —
 *  the server is authoritative, so the チャット toggle renders this rather
 *  than its own local state. */
export interface VoiceMessage {
  type: "voice";
  active: boolean;
}

export interface ErrorMessage {
  type: "error";
  message: string;
}

export interface InputAcceptedMessage {
  type: "inputAccepted";
  requestId: string;
}

export interface ResponseMessage {
  type: "response";
  requestId: string;
  status: "done" | "error";
  text?: string;
  message?: string;
}

/** One fact learned about a person, as recorded on their `PersonRecord`. */
export interface PersonFactRecord {
  text: string;
  source: string; // "chat" | "vision" | "manual"
  createdAt: number; // unix秒
}

/** A person tracked across chat/vision/event sources — the camelCase wire
 *  form of the Rust `Person` (see `crates/npc-core/src/person.rs`). */
export interface PersonRecord {
  id: string;
  name: string;
  aliases: string[];
  firstSeen: number; // unix秒
  lastSeen: number; // unix秒
  encounterCount: number;
  appearance: string;
  facts: PersonFactRecord[];
  familiarity: number; // 0..1
  source: string; // "chat" | "vision" | "event" | "manual"
  notes: string;
}

/** A person record was created or updated. */
export interface PersonMessage {
  type: "person";
  person: PersonRecord;
}

/** A person record was deleted. */
export interface PersonDeletedMessage {
  type: "personDeleted";
  id: string;
}

/**
 * Acknowledges an `event` frame was received and queued. This exists for
 * out-of-process extension clients -- bridges that fire `follow`/`resub`/etc.
 * frames over this same WS protocol (see
 * tc-assistant2/docs/extension-api-spec.md, which this protocol is being
 * extended to be a strict superset of) -- that have no other feed telling
 * them the event actually reached the server. `kind` echoes the event
 * frame's own `kind` so a bridge firing several kinds can match each ack to
 * its send without also carrying a request id.
 *
 * This UI never sends `event` frames itself -- events arrive from external
 * integrations, not from a person typing into this chat -- so there is
 * nothing for useNpcSocket to *do* with this beyond keeping it out of the
 * `default` arm of its switch; see that handler's comment.
 */
export interface EventAcceptedMessage {
  type: "eventAccepted";
  kind: string;
}

/**
 * Acknowledges an `interrupt` frame was applied -- again primarily for
 * extension clients, which (unlike this UI) have no other signal that the
 * interrupt landed. This UI already observes an interrupt's effect directly
 * the moment it happens: the in-flight reply's `chat`/`silent` frame simply
 * never arrives and `speaking` drops to false, so a dedicated ack has
 * nothing to add here.
 */
export interface InterruptAcceptedMessage {
  type: "interruptAccepted";
}

/**
 * Acknowledges a `suspend` frame was applied. Unlike `interruptAccepted`,
 * this pair (with `ResumeAcceptedMessage`) is genuinely useful to this UI and
 * not just to extension clients: the チャット sidebar's 音声 panel sends
 * `suspend`/`resume` from its Pause/Resume buttons (see VoicePanel.tsx) but
 * previously had no feedback at all that a click had reached the server --
 * it fired the frame and moved on. The existing `voice` frame doesn't cover
 * this: that one reports the position of the master mic-to-speaker loop
 * switch (`voiceStart`/`voiceStop`), an unrelated on/off knob from this
 * TTS-only pause. See useNpcSocket's `ttsSuspended` for how this is used,
 * including the one case it can't track: npc-speech's own
 * auto-resume-after-timeout, which is not a reply to a client `resume` frame
 * and so isn't guaranteed to arrive with a matching `resumeAccepted`.
 */
export interface SuspendAcceptedMessage {
  type: "suspendAccepted";
}

/** The `resume` counterpart of `SuspendAcceptedMessage` -- see its comment. */
export interface ResumeAcceptedMessage {
  type: "resumeAccepted";
}

export type ServerMessage =
  | HelloMessage
  | ChatMessage
  | SilentMessage
  | TtsLineMessage
  | SenseMessage
  | TranslationMessage
  | MemoryMessage
  | AffectMessage
  | ActionLogMessage
  | PositionMessage
  | VolumeMessage
  | SpeakingMessage
  | SpeakingLevelMessage
  | AvatarMessage
  | StatusMessage
  | VoiceMessage
  | ErrorMessage
  | InputAcceptedMessage
  | ResponseMessage
  | PersonMessage
  | PersonDeletedMessage
  | EventAcceptedMessage
  | InterruptAcceptedMessage
  | SuspendAcceptedMessage
  | ResumeAcceptedMessage;

export type ClientMessage =
  | { type: "input"; text: string; speaker?: string }
  | { type: "command"; text: string }
  | { type: "interrupt" }
  | { type: "suspend" }
  | { type: "resume" }
  | { type: "voiceStart" }
  | { type: "voiceStop" }
  | {
      type: "event";
      kind: string;
      userName?: string;
      text?: string;
      amount?: number;
      /** Subscription tier (e.g. "1000"/"2000"/"3000") -- meaningful only
       *  for `kind: "resub"`. Plain on the wire, unlike `rewardTitle` below. */
      tier?: string;
      /** Free-text message carried by the event, e.g. a resub's carried-over
       *  message. */
      message?: string;
      /** Reward name for a channel-points-style redemption event. camelCase
       *  on the wire (unlike `tier`/`message`) because it mirrors the
       *  extension API spec's own field name exactly, so a bridge that
       *  already emits `rewardTitle` needs no translation layer to talk to
       *  this server. */
      rewardTitle?: string;
    };

// --- REST -------------------------------------------------------------

export interface CharacterSummary {
  id: string;
  name: string;
  active: boolean;
  /** Assigned VRM, or null. Carried on the list itself so the キャラ tab can
   *  show each character's model without a per-character fetch. */
  avatar?: AvatarRef | null;
}

/** One `.vrm` in the server's model folder (`GET /api/vrm`). The bytes are
 *  fetched separately, per model actually displayed. */
export interface VrmModel {
  /** File name including the extension — the model's identity. */
  file: string;
  /** File name without the extension, for display. */
  name: string;
  size: number;
}

/** Opaque config blob — GET /api/config masks api keys as "***"; PUT sends
 *  the full (edited) JSON back and the server preserves any key still left
 *  as "***". The web UI treats this as an untyped JSON document (see
 *  views/SettingsView.tsx). */
export type ConfigDocument = Record<string, unknown>;

/** GET /api/memory — a snapshot of the memory module's current state. */
export interface MemoryDocument {
  shortTerm: string;
  longTerm: { docId: string; text: string; createdAt: string; personId: string }[];
}

/** GET /api/people/:id — a person plus their long-term memories, newest
 *  first, capped at 50 on the server. */
export interface PersonDetail {
  person: PersonRecord;
  memories: { docId: string; text: string; createdAt: string }[];
}

/**
 * One entry of `GET /api/affect/history` — the same fields as
 * `AffectMessage` minus the `type` tag (every entry in the array is affect
 * data, so no per-entry discriminant is needed). `partner` is omitted
 * (rather than `null`) exactly like the WS frame, for the same reason:
 * nobody has identified themselves for that snapshot.
 */
export interface AffectHistoryEntry {
  ts: number;
  familiarity: number;
  closing: boolean;
  inviteCaution: boolean;
  partner?: string;
  partnerKnown: boolean;
  partnerSwitched: boolean;
  partnerAway: boolean;
  drives: DriveState[];
}

/**
 * GET /api/affect/history — the server's rolling buffer of recent `affect`
 * snapshots, oldest first. Exists so the 感情 tab's trend sparkline can seed
 * itself on page load: `useNpcSocket`'s `affectHistory` is filled only by
 * live WS frames and is always empty right after a reload.
 */
export interface AffectHistoryDocument {
  entries: AffectHistoryEntry[];
}
