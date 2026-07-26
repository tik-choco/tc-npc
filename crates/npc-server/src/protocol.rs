//! WS frame shapes exchanged with the web UI (`web/src/lib/types.ts` is the
//! contract this module implements — field names and tag values must match
//! exactly).

use serde::{Deserialize, Serialize};

/// `Record<string, boolean>` on the TS side — plain struct fields serialize
/// to exactly that shape.
#[derive(Debug, Clone, Serialize)]
pub struct ModuleFlags {
    pub talk: bool,
    pub memory: bool,
    pub speech: bool,
    pub vision: bool,
    pub action: bool,
    pub scheduler: bool,
    /// `config.translation.mode` is not `off` — i.e. the 通訳 tab is live.
    pub translation: bool,
}

#[derive(Debug, Clone, Serialize)]
pub struct CharacterRef {
    pub id: String,
    pub name: String,
}

/// One drive's current reading within an `affect` frame's `drives` array.
/// `key` is the contract's snake_case wire key (see
/// `npc_talk::affect::DriveKey::as_str`), e.g. `"substance_p"`.
#[derive(Debug, Clone, Serialize)]
pub struct DriveLevel {
    pub key: String,
    pub level: f32,
    pub base: f32,
}

/// Server -> client frames. Internally tagged on `"type"`, matching
/// `ServerMessage` in `web/src/lib/types.ts`.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ServerMsg {
    #[serde(rename = "hello")]
    Hello {
        version: String,
        modules: ModuleFlags,
        character: Option<CharacterRef>,
    },
    #[serde(rename = "chat")]
    Chat { role: String, text: String, ts: i64 },
    #[serde(rename = "ttsLine")]
    TtsLine {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        translations: Option<serde_json::Value>,
    },
    #[serde(rename = "sense")]
    Sense { kind: String, text: String, ts: i64 },
    /// One simultaneous-interpretation update from npc-translate. `lang` is
    /// empty for the "heard this line, translations pending" frame and names
    /// the target language for each finished translation; all frames for one
    /// utterance share an `id` so the client groups them.
    #[serde(rename = "translation")]
    Translation {
        id: String,
        source: String,
        original: String,
        lang: String,
        text: String,
        reversed: bool,
        ts: i64,
    },
    #[serde(rename = "memory")]
    Memory { kind: String, text: String },
    /// The NPC's internal affect/drive state, published by npc-talk after
    /// every conversation turn (see `npc_talk::affect::AffectState::snapshot`).
    #[serde(rename = "affect")]
    Affect {
        ts: i64,
        familiarity: f32,
        closing: bool,
        #[serde(rename = "inviteCaution")]
        invite_caution: bool,
        drives: Vec<DriveLevel>,
    },
    #[serde(rename = "actionLog")]
    ActionLog { text: String },
    #[serde(rename = "position")]
    Position { x: f64, y: f64, heading: f64 },
    #[serde(rename = "volume")]
    Volume { level: f64 },
    #[allow(dead_code)]
    #[serde(rename = "status")]
    Status { modules: ModuleFlags },
    #[serde(rename = "error")]
    Error { message: String },
    #[serde(rename = "inputAccepted")]
    InputAccepted {
        #[serde(rename = "requestId")]
        request_id: String,
    },
    #[serde(rename = "response")]
    Response {
        #[serde(rename = "requestId")]
        request_id: String,
        status: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        text: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    /// A person record was created or updated. `person` is the camelCase
    /// `PersonRecord` shape produced by `npc_core::person_to_wire` (see the
    /// person-memory contract §5) — passed through as-is rather than
    /// re-modeled here since npc-server only forwards it.
    #[serde(rename = "person")]
    Person { person: serde_json::Value },
    /// A person record was deleted.
    #[serde(rename = "personDeleted")]
    PersonDeleted { id: String },
}

/// Client -> server frames. Internally tagged on `"type"`, matching
/// `ClientMessage` in `web/src/lib/types.ts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    #[serde(rename = "input")]
    Input {
        text: String,
        /// Optional display name of whoever typed `text`, so the bus
        /// message can be attributed to a person record (see the
        /// person-memory contract §4). Absent/`None` leaves the payload
        /// exactly as before (no `speaker` field).
        #[serde(default)]
        speaker: Option<String>,
    },
    #[serde(rename = "command")]
    Command { text: String },
    #[serde(rename = "interrupt")]
    Interrupt,
    #[serde(rename = "suspend")]
    Suspend,
    #[serde(rename = "resume")]
    Resume,
    #[serde(rename = "event")]
    Event {
        kind: String,
        #[serde(rename = "userName", default)]
        user_name: Option<String>,
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        amount: Option<f64>,
    },
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}
