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

export interface MemoryMessage {
  type: "memory";
  kind: "short" | "long";
  text: string;
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

export type ServerMessage =
  | HelloMessage
  | ChatMessage
  | TtsLineMessage
  | SenseMessage
  | MemoryMessage
  | ActionLogMessage
  | PositionMessage
  | VolumeMessage
  | StatusMessage
  | ErrorMessage
  | InputAcceptedMessage
  | ResponseMessage;

export type ClientMessage =
  | { type: "input"; text: string }
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
