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
}

#[derive(Debug, Clone, Serialize)]
pub struct CharacterRef {
    pub id: String,
    pub name: String,
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
    #[serde(rename = "memory")]
    Memory { kind: String, text: String },
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
}

/// Client -> server frames. Internally tagged on `"type"`, matching
/// `ClientMessage` in `web/src/lib/types.ts`.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    #[serde(rename = "input")]
    Input { text: String },
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
