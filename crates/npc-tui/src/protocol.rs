//! Wire types for the `/ws` endpoint this crate speaks against, mirroring
//! `npc_server::protocol` (see `crates/npc-server/src/protocol.rs`) — that
//! module and `web/src/lib/types.ts` are the actual contract; this is a
//! deliberately partial client-side reflection of it, not a full port.
//!
//! Two things make this a *reflection* rather than a copy:
//!
//! 1. Only the frame kinds the TUI's v1 scope (chat / status / log / quit)
//!    needs are given real variants. Everything else — `ttsLine`,
//!    `translation`, `affect`, `position`, `volume`, `speaking`,
//!    `speakingLevel`, `avatar`, `person`, `personDeleted`, the
//!    suspend/resume/voice ack quartet — lands in [`ServerMsg::Unknown`]
//!    instead of a parse error. A TUI has no VRM to animate and no 感情
//!    sparkline to plot, so there is nothing productive to do with those
//!    frames beyond "don't crash on them" — and `Unknown` is exactly that,
//!    forwarded to the log panel as `type: <name>` so a user can still see
//!    that *something* arrived.
//! 2. This is read-only where npc-server's is read/write: only
//!    [`ClientMsg::Input`] exists, because v1 only ever sends chat text (see
//!    the worker brief's scope list — no suspend/resume/voice toggles, no
//!    viewer-event forwarding, all of which belong to a browser tab or
//!    extension, not an operator's terminal).
//!
//! Keeping this partial (rather than porting the whole enum) is deliberate
//! insulation against server-side evolution: a new frame kind lighting up in
//! `npc-server`'s `ServerMsg` should never require an in-lockstep change here
//! just to keep compiling — it should simply show up as `Unknown` until
//! someone decides the TUI actually needs to render it.

use serde::{Deserialize, Serialize};

/// Mirrors `npc_server::protocol::ModuleFlags` field-for-field (plain struct
/// fields, no renames on either side, so the JSON shape lines up without any
/// `#[serde(rename)]` here either).
#[derive(Debug, Clone, Default, Deserialize)]
pub struct ModuleFlags {
    pub talk: bool,
    pub memory: bool,
    pub speech: bool,
    pub vision: bool,
    pub action: bool,
    pub scheduler: bool,
    pub translation: bool,
}

/// Mirrors `npc_server::protocol::CharacterRef`.
#[derive(Debug, Clone, Deserialize)]
pub struct CharacterRef {
    pub id: String,
    pub name: String,
}

/// Server -> client frames actually consumed by this crate. Internally
/// tagged on `"type"`, matching the wire shape `npc-server` sends — but see
/// the module doc for why this enum is a subset with a catch-all, not a full
/// port of `npc_server::protocol::ServerMsg`.
#[derive(Debug, Clone, Deserialize)]
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
    /// A conversation turn the NPC deliberately left unanswered — see
    /// `npc_server::protocol::ServerMsg::Silent`'s doc for why this stands in
    /// for `chat` rather than being folded into it.
    #[serde(rename = "silent")]
    Silent { reason: String, ts: i64 },
    #[serde(rename = "sense")]
    Sense { kind: String, text: String, ts: i64 },
    #[serde(rename = "memory")]
    Memory { kind: String, text: String },
    #[serde(rename = "actionLog")]
    ActionLog { text: String },
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
        #[serde(default)]
        text: Option<String>,
        #[serde(default)]
        message: Option<String>,
    },
    /// Catch-all for every frame kind this crate doesn't render (see the
    /// module doc). `serde(other)` matches any `"type"` value not named
    /// above — including ones that don't exist yet — and discards the rest
    /// of the object; the log panel is expected to report frames of this
    /// variant by kind name only, since only [`handle_client_message`]'s
    /// counterpart on the server side knows the rest of their shape.
    ///
    /// The `String` is filled in by [`ServerMsg::type_name`] reading the raw
    /// JSON's `"type"` field back out, not by serde itself — `#[serde(other)]`
    /// only marks *that* this variant is the fallback, it doesn't hand the
    /// unmatched tag value to the variant. See `parse` below.
    Unknown(String),
}

impl ServerMsg {
    /// Parse one WS text frame. Unlike a bare `serde_json::from_str`, an
    /// unrecognized `"type"` becomes `Ok(ServerMsg::Unknown(name))` instead of
    /// `Err` — see the module doc for why silently degrading is the right
    /// call for a client that only implements part of the protocol. A frame
    /// that isn't even a JSON object with a string `"type"` field is still a
    /// hard parse error: that's not "a frame kind we don't handle", it's the
    /// server (or something claiming to be it) sending something malformed.
    pub fn parse(raw: &str) -> Result<ServerMsg, serde_json::Error> {
        match serde_json::from_str::<ServerMsg>(raw) {
            Ok(msg) => Ok(msg),
            Err(err) => {
                // `serde(other)` needs a *unit* fallback variant to compile,
                // but ServerMsg::Unknown carries a String, so it can't use
                // that mechanism directly. Recover the tag by hand instead:
                // if this parses as a plain JSON object with a string
                // "type", that's an unknown-but-well-formed frame; anything
                // else re-raises the original serde_json error untouched, so
                // a caller still sees a real parse failure for genuinely
                // malformed input. (A second `from_str` call rather than
                // reusing `err` — `serde_json::Error` isn't `Clone` — is a
                // non-issue: reparsing a single WS text frame as a
                // `Value` is negligible next to the network hop that
                // delivered it.)
                match serde_json::from_str::<serde_json::Value>(raw) {
                    Ok(value) => match value.get("type").and_then(serde_json::Value::as_str) {
                        Some(kind) => Ok(ServerMsg::Unknown(kind.to_string())),
                        None => Err(err),
                    },
                    Err(_) => Err(err),
                }
            }
        }
    }
}

/// Client -> server frames this crate sends. See the module doc for why this
/// is `Input` only — every other `ClientMsg` variant in `npc-server`
/// (`command`, `interrupt`, `suspend`/`resume`, `voiceStart`/`voiceStop`,
/// `event`) belongs to a browser tab or streaming extension, not a plain
/// chat terminal.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type")]
pub enum ClientMsg {
    #[serde(rename = "input")]
    Input {
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        speaker: Option<String>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hello() {
        let raw = r#"{"type":"hello","version":"1","modules":{"talk":true,"memory":false,"speech":true,"vision":false,"action":true,"scheduler":false,"translation":false},"character":{"id":"c1","name":"Test"},"avatar":{"kind":"vrm","file":"a.vrm"}}"#;
        let msg = ServerMsg::parse(raw).unwrap();
        match msg {
            ServerMsg::Hello {
                version,
                modules,
                character,
            } => {
                assert_eq!(version, "1");
                assert!(modules.talk);
                assert!(!modules.memory);
                assert_eq!(character.unwrap().name, "Test");
            }
            other => panic!("expected Hello, got {other:?}"),
        }
    }

    /// `hello`'s `avatar` field (a nested object this crate has no struct
    /// for) must not break parsing of the fields it does care about — this
    /// is what actually exercises "extra unknown fields inside a known
    /// variant are ignored", as opposed to `Unknown`'s "unknown `type`
    /// entirely" case below.
    #[test]
    fn ignores_unmodeled_fields_within_a_known_variant() {
        let raw = r#"{"type":"chat","role":"assistant","text":"hi","ts":1,"extraFieldFromANewerServer":true}"#;
        let msg = ServerMsg::parse(raw).unwrap();
        assert!(matches!(msg, ServerMsg::Chat { .. }));
    }

    #[test]
    fn unrecognized_type_becomes_unknown_not_an_error() {
        let raw = r#"{"type":"speakingLevel","level":0.5}"#;
        let msg = ServerMsg::parse(raw).unwrap();
        match msg {
            ServerMsg::Unknown(kind) => assert_eq!(kind, "speakingLevel"),
            other => panic!("expected Unknown, got {other:?}"),
        }
    }

    #[test]
    fn malformed_frame_is_still_a_hard_error() {
        let raw = "not json at all";
        assert!(ServerMsg::parse(raw).is_err());
    }

    #[test]
    fn frame_without_a_type_field_is_still_a_hard_error() {
        let raw = r#"{"role":"user","text":"hi"}"#;
        assert!(ServerMsg::parse(raw).is_err());
    }

    #[test]
    fn silent_frame_round_trips() {
        let raw = r#"{"type":"silent","reason":"declined","ts":123}"#;
        let msg = ServerMsg::parse(raw).unwrap();
        match msg {
            ServerMsg::Silent { reason, ts } => {
                assert_eq!(reason, "declined");
                assert_eq!(ts, 123);
            }
            other => panic!("expected Silent, got {other:?}"),
        }
    }

    #[test]
    fn response_error_status_carries_message() {
        let raw = r#"{"type":"response","requestId":"r1","status":"error","message":"boom"}"#;
        let msg = ServerMsg::parse(raw).unwrap();
        match msg {
            ServerMsg::Response {
                request_id,
                status,
                text,
                message,
            } => {
                assert_eq!(request_id, "r1");
                assert_eq!(status, "error");
                assert_eq!(text, None);
                assert_eq!(message.as_deref(), Some("boom"));
            }
            other => panic!("expected Response, got {other:?}"),
        }
    }

    #[test]
    fn client_input_without_speaker_omits_the_field() {
        let msg = ClientMsg::Input {
            text: "こんにちは".to_string(),
            speaker: None,
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "input", "text": "こんにちは" })
        );
    }

    #[test]
    fn client_input_with_speaker_includes_it() {
        let msg = ClientMsg::Input {
            text: "hi".to_string(),
            speaker: Some("op".to_string()),
        };
        let json = serde_json::to_value(&msg).unwrap();
        assert_eq!(
            json,
            serde_json::json!({ "type": "input", "text": "hi", "speaker": "op" })
        );
    }
}
