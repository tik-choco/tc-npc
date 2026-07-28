//! Forwards `npc_core::Bus` traffic to connected WS clients. One task,
//! spawned once at server startup, subscribes to the whole bus (subscribers
//! receive every topic and filter) and translates recognized envelopes into
//! `ServerMsg` frames broadcast via the [`Hub`].

use std::sync::{Arc, Mutex};

use npc_core::{msg, topic, BusMessage, ModuleCtx};
use serde_json::Value;

use crate::hub::{EchoGuard, Hub, RateLimiter};
use crate::protocol::{now_ms, DriveLevel, ServerMsg};

/// Topic used by other modules (npc-action, npc-vision, npc-speech,
/// npc-translate, ...) to
/// push UI-only updates that don't fit the existing `agent:*` topics (e.g.
/// avatar position, current TTS playback line, a raw action log line). Not
/// part of `npc_core::bus::topic` since it's consumed only by npc-server.
const UI_TOPIC: &str = "npc:ui";

pub fn spawn(ctx: ModuleCtx, hub: Hub, echo: std::sync::Arc<EchoGuard>, short_term_memory: Arc<Mutex<String>>) {
    let mut rx = ctx.bus.subscribe();
    let shutdown = ctx.shutdown.clone();
    let volume_limiter = RateLimiter::per_second(10);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                received = rx.recv() => {
                    match received {
                        Ok(bus_msg) => handle(&hub, &echo, &volume_limiter, &short_term_memory, bus_msg),
                        Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "npc-server: bus forwarder lagged, dropped messages");
                        }
                        Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });
}

fn handle(
    hub: &Hub,
    echo: &EchoGuard,
    volume_limiter: &RateLimiter,
    short_term_memory: &Arc<Mutex<String>>,
    bus_msg: BusMessage,
) {
    let BusMessage { topic: bus_topic, env } = bus_msg;

    match bus_topic.as_str() {
        t if t == topic::CHAT => {
            if env.r#type == msg::CHAT_RESPONSE {
                let content = text_field(&env.payload).unwrap_or_default();
                hub.broadcast(&ServerMsg::Chat {
                    role: "assistant".to_string(),
                    text: content.clone(),
                    ts: now_ms(),
                });
                hub.broadcast(&ServerMsg::TtsLine {
                    text: content.clone(),
                    translations: None,
                });
                hub.broadcast(&ServerMsg::Response {
                    request_id: "-".to_string(),
                    status: "done".to_string(),
                    text: Some(content),
                    message: None,
                });
            }
        }
        t if t == topic::SENSE => {
            let kind = match env.r#type.as_str() {
                msg::VISION => "vision",
                msg::SPEECH => "speech",
                _ => return,
            };
            let content = text_field(&env.payload).unwrap_or_default();
            if kind == "speech" && echo.was_recent(&content) {
                // Already shown as a `chat` frame from the WS input/event
                // that produced it; skip the duplicate `sense` frame.
                return;
            }
            hub.broadcast(&ServerMsg::Sense {
                kind: kind.to_string(),
                text: content,
                ts: now_ms(),
            });
        }
        t if t == topic::MEM => {
            let kind = match env.r#type.as_str() {
                msg::SHORT_TERM_MEMORY => "short",
                msg::LONG_TERM_MEMORY => "long",
                _ => return,
            };
            let content = text_field(&env.payload).unwrap_or_default();
            if kind == "short" {
                // Cached so `GET /api/memory` can answer without the REST
                // handler needing its own bus subscription.
                *short_term_memory.lock().unwrap() = content.clone();
            }
            hub.broadcast(&ServerMsg::Memory {
                kind: kind.to_string(),
                text: content,
            });
        }
        UI_TOPIC => handle_ui(hub, volume_limiter, env.r#type.as_str(), &env.payload),
        // `topic::CONFIG` ("npc:config") is deliberately NOT matched here: its
        // payload is the full, unredacted Config (including api_key secrets)
        // and must never reach a WS client. Only topics explicitly handled
        // above are forwarded — this match is a whitelist, not a blocklist.
        _ => {}
    }
}

fn handle_ui(hub: &Hub, volume_limiter: &RateLimiter, msg_type: &str, payload: &Value) {
    match msg_type {
        "volume" => {
            if !volume_limiter.allow() {
                return;
            }
            if let Some(level) = payload.get("level").and_then(Value::as_f64) {
                hub.broadcast(&ServerMsg::Volume { level });
            }
        }
        "position" => {
            let x = payload.get("x").and_then(Value::as_f64).unwrap_or(0.0);
            let y = payload.get("y").and_then(Value::as_f64).unwrap_or(0.0);
            let heading = payload.get("heading").and_then(Value::as_f64).unwrap_or(0.0);
            hub.broadcast(&ServerMsg::Position { x, y, heading });
        }
        "action_log" => {
            let text = text_field(payload).unwrap_or_default();
            hub.broadcast(&ServerMsg::ActionLog { text });
        }
        "tts_line" => {
            let text = text_field(payload).unwrap_or_default();
            hub.broadcast(&ServerMsg::TtsLine {
                text,
                translations: None,
            });
        }
        msg::AFFECT_STATE => {
            match serde_json::from_value::<AffectStatePayload>(payload.clone()) {
                Ok(p) => {
                    hub.broadcast(&ServerMsg::Affect {
                        ts: now_ms(),
                        familiarity: p.familiarity,
                        closing: p.closing,
                        invite_caution: p.invite_caution,
                        partner: p.partner,
                        partner_known: p.partner_known,
                        partner_switched: p.partner_switched,
                        partner_away: p.partner_away,
                        drives: p
                            .drives
                            .into_iter()
                            .map(|d| DriveLevel {
                                key: d.key,
                                level: d.level,
                                base: d.base,
                            })
                            .collect(),
                    });
                }
                Err(err) => {
                    tracing::warn!(error = %err, "npc-server: failed to parse affect_state payload");
                }
            }
        }
        msg::PERSON_UPDATED => {
            hub.broadcast(&ServerMsg::Person {
                person: payload.clone(),
            });
        }
        msg::PERSON_DELETED => {
            if let Some(id) = payload.get("id").and_then(Value::as_str) {
                hub.broadcast(&ServerMsg::PersonDeleted { id: id.to_string() });
            }
        }
        "translation" => {
            let str_field = |key: &str| {
                payload
                    .get(key)
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            };
            hub.broadcast(&ServerMsg::Translation {
                id: str_field("id"),
                source: str_field("source"),
                original: str_field("original"),
                lang: str_field("lang"),
                text: str_field("text"),
                reversed: payload.get("reversed").and_then(Value::as_bool).unwrap_or(false),
                ts: payload
                    .get("ts")
                    .and_then(Value::as_i64)
                    .unwrap_or_else(now_ms),
            });
        }
        _ => {}
    }
}

/// Deserialization shape for `npc:ui` / `affect_state` payloads, matching
/// `npc_talk::affect::AffectSnapshot`'s camelCase JSON (npc-server does not
/// depend on npc-talk, so this is a minimal standalone mirror rather than a
/// shared type).
#[derive(Debug, serde::Deserialize)]
struct AffectStatePayload {
    #[serde(default)]
    familiarity: f32,
    #[serde(default)]
    closing: bool,
    #[serde(default, rename = "inviteCaution")]
    invite_caution: bool,
    #[serde(default)]
    partner: Option<String>,
    #[serde(default, rename = "partnerKnown")]
    partner_known: bool,
    #[serde(default, rename = "partnerSwitched")]
    partner_switched: bool,
    #[serde(default, rename = "partnerAway")]
    partner_away: bool,
    #[serde(default)]
    drives: Vec<AffectDrivePayload>,
}

#[derive(Debug, serde::Deserialize)]
struct AffectDrivePayload {
    key: String,
    level: f32,
    base: f32,
}

/// Publishers use `{"content": ...}` (matching what this crate publishes for
/// WS `input`/`event` frames); fall back to `{"text": ...}` for producers
/// that use that name instead.
fn text_field(payload: &Value) -> Option<String> {
    payload
        .get("content")
        .and_then(Value::as_str)
        .or_else(|| payload.get("text").and_then(Value::as_str))
        .map(|s| s.to_string())
}
