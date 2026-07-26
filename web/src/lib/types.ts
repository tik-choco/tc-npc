// WS protocol contract shared with the Rust server (crates/npc-server). Keep
// this in sync with the server side — it is intentionally the single source
// of truth for frame shapes on the web UI.

export interface HelloMessage {
  type: "hello";
  version: string;
  modules: Record<string, boolean>;
  character: { id: string; name: string } | null;
}

export interface ChatMessage {
  type: "chat";
  role: "user" | "assistant";
  text: string;
  ts: number;
}

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
 */
export interface AffectMessage {
  type: "affect";
  ts: number;
  familiarity: number;
  closing: boolean;
  inviteCaution: boolean;
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

export interface StatusMessage {
  type: "status";
  modules: Record<string, boolean>;
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

export type ServerMessage =
  | HelloMessage
  | ChatMessage
  | TtsLineMessage
  | SenseMessage
  | TranslationMessage
  | MemoryMessage
  | AffectMessage
  | ActionLogMessage
  | PositionMessage
  | VolumeMessage
  | StatusMessage
  | ErrorMessage
  | InputAcceptedMessage
  | ResponseMessage
  | PersonMessage
  | PersonDeletedMessage;

export type ClientMessage =
  | { type: "input"; text: string; speaker?: string }
  | { type: "command"; text: string }
  | { type: "interrupt" }
  | { type: "suspend" }
  | { type: "resume" }
  | { type: "event"; kind: string; userName?: string; text?: string; amount?: number };

// --- REST -------------------------------------------------------------

export interface CharacterSummary {
  id: string;
  name: string;
  active: boolean;
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
