// Typed views over the sections of the server config document that the UI
// edits (schedule, action map, AI settings). The Rust side
// (crates/npc-core/src/config.rs) owns the schema; these mirror only the
// fields the UI touches. Everything is optional because the server applies
// serde defaults — always spread the existing section when writing back so
// unknown/unedited fields survive the round-trip.

/**
 * config.scheduler.announcements[].actions[] — extra bus messages an
 * announcement publishes when it fires (the port of the Go scheduler's
 * `redis_actions`). Tagged on `kind`; see `ScheduledAction` in
 * crates/npc-core/src/config.rs for what each one publishes.
 */
export type ScheduledActionKind = "speak" | "action" | "command" | "chat" | "suspend" | "resume" | "raw";

export type ScheduledActionEntry =
  | { kind: "speak"; content?: string; chime_file?: string }
  | { kind: "action"; content?: string }
  | { kind: "command"; text?: string }
  | { kind: "chat"; content?: string }
  | { kind: "suspend" }
  | { kind: "resume" }
  | { kind: "raw"; topic: string; type: string; payload?: unknown };

/** config.scheduler.announcements[] — daily "HH:MM" / "HH:MM:SS" repeats. */
export interface AnnouncementEntry {
  time: string;
  text: string;
  chime_file?: string;
  volume?: number;
  actions?: ScheduledActionEntry[];
}

export interface SchedulerSection {
  enabled?: boolean;
  announcements?: AnnouncementEntry[];
}

/** config.translation — simultaneous interpretation (通訳タブ). */
export interface TranslationSection {
  /** "off" | "interpret" | "assist" — anything else is treated as "off". */
  mode?: string;
  source_language?: string;
  target_language?: string;
  target_language_2?: string;
  context_size?: number;
  auto_reverse?: boolean;
  /** Send subtitles to the VRChat chatbox (uses vrc.osc_address). */
  chatbox?: boolean;
  /** Model override; empty falls back to api.model. */
  model?: string;
  /** "" follows default_preset_id (lib/llm-config.ts's resolvePreset). */
  preset_id?: string;
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
  /** "" follows default_preset_id (lib/llm-config.ts's resolvePreset). */
  preset_id?: string;
}

/** config.api — OpenAI-compatible chat/embedding connection. */
export interface ApiSection {
  base_url?: string;
  /** Masked as "***" by GET /api/config; send "***" back to keep the saved value. */
  api_key?: string;
  model?: string;
  embedding_model?: string;
  /** "none" | "minimal" | "low" | "medium" | "high" — always sent on chat
   * requests ("none" is an explicit value, not "omit"); empty/missing is
   * treated as "none" by the server. */
  reasoning_effort?: string;
  /** "" follows default_preset_id (lib/llm-config.ts's resolvePreset). */
  preset_id?: string;
}

/** config.speech — local audio I/O: which mic/speaker the server opens, and
 * the sample rates it resamples to. Device fields hold a device *name*, which
 * the server matches case-insensitively as a substring; "" means "whatever
 * the OS calls default". A change here restarts just the affected audio
 * thread on the server, so it takes effect without an app restart. */
export interface SpeechSection {
  input_device?: string;
  output_device?: string;
  input_sample_rate?: number;
  output_sample_rate?: number;
  [key: string]: unknown;
}

/** config.tts / config.stt — speech synthesis / recognition connections. */
export interface SpeechEndpointSection {
  enabled?: boolean;
  base_url?: string;
  api_key?: string;
  model?: string;
  voice?: string;
  /** "" follows default_preset_id (lib/llm-config.ts's resolvePreset). */
  preset_id?: string;
  [key: string]: unknown;
}

/** config.providers[] — "where to connect" (lib/llm-config.ts's shared
 * provider/preset model, tc-docs/drafts/llm-settings-common-v1.md §2). */
export interface ProviderEntry {
  id: string;
  label?: string;
  base_url?: string;
  /** Masked as "***" by GET /api/config; send "***" back to keep the saved value. */
  api_key?: string;
}

/** config.presets[] — "how to call it": a model + reasoning effort bound to
 * a provider. `reasoning_effort: ""` inherits config.api's reasoning_effort
 * (lib/llm-config.ts). */
export interface PresetEntry {
  id: string;
  label?: string;
  provider_id?: string;
  model?: string;
  reasoning_effort?: string;
}

/** config.talk — chat/conversation task assignment. */
export interface TalkSection {
  /** "" follows default_preset_id (lib/llm-config.ts's resolvePreset). */
  preset_id?: string;
}

/** config.memory — memory task + its embedding task, assigned independently. */
export interface MemorySection {
  enabled?: boolean;
  /** "" follows default_preset_id (lib/llm-config.ts's resolvePreset). */
  preset_id?: string;
  /** "" follows default_preset_id, same as preset_id. */
  embedding_preset_id?: string;
  [key: string]: unknown;
}

/** config.vision — image-understanding task; has its own OpenAI-compatible
 * connection fields alongside the shared preset assignment (mirrors
 * SpeechEndpointSection's fallback-to-fields shape). */
export interface VisionSection {
  enabled?: boolean;
  base_url?: string;
  api_key?: string;
  model?: string;
  /** "" follows default_preset_id (lib/llm-config.ts's resolvePreset). */
  preset_id?: string;
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
