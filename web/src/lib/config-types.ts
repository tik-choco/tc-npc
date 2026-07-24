// Typed views over the sections of the server config document that the UI
// edits (schedule, action map, AI settings). The Rust side
// (crates/npc-core/src/config.rs) owns the schema; these mirror only the
// fields the UI touches. Everything is optional because the server applies
// serde defaults — always spread the existing section when writing back so
// unknown/unedited fields survive the round-trip.

/** config.scheduler.announcements[] — daily "HH:MM" / "HH:MM:SS" repeats. */
export interface AnnouncementEntry {
  time: string;
  text: string;
  chime_file?: string;
  volume?: number;
}

export interface SchedulerSection {
  enabled?: boolean;
  announcements?: AnnouncementEntry[];
}

/** config.action.locations[] — a named point in VRChat-world coordinates. */
export interface LocationEntry {
  name: string;
  x?: number;
  y?: number;
  heading?: number;
}

/** config.action.routes[].waypoints[] — `location` refers to LocationEntry.name. */
export interface WaypointEntry {
  location: string;
  seconds?: number;
  /** When true the NPC pauses (a fixed 2 s on the server) at this waypoint. */
  wait?: boolean;
}

export interface RouteEntry {
  name: string;
  loop?: boolean;
  waypoints?: WaypointEntry[];
}

export interface ActionSection {
  enabled?: boolean;
  osc_address?: string;
  locations?: LocationEntry[];
  routes?: RouteEntry[];
}

/** config.api — OpenAI-compatible chat/embedding connection. */
export interface ApiSection {
  base_url?: string;
  /** Masked as "***" by GET /api/config; send "***" back to keep the saved value. */
  api_key?: string;
  model?: string;
  embedding_model?: string;
  reasoning_effort?: string;
}

/** config.tts / config.stt — speech synthesis / recognition connections. */
export interface SpeechEndpointSection {
  enabled?: boolean;
  base_url?: string;
  api_key?: string;
  model?: string;
  voice?: string;
  [key: string]: unknown;
}

export interface VrcSection {
  chatbox?: boolean;
  osc_address?: string;
}

/** Sections whose only UI-relevant knob is the module on/off switch. */
export interface EnabledSection {
  enabled?: boolean;
  [key: string]: unknown;
}
