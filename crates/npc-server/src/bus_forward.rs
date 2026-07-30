//! Forwards `npc_core::Bus` traffic to connected WS clients. One task,
//! spawned once at server startup, subscribes to the whole bus (subscribers
//! receive every topic and filter) and translates recognized envelopes into
//! `ServerMsg` frames broadcast via the [`Hub`].

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use npc_core::{msg, topic, BusMessage, ModuleCtx};
use serde_json::Value;

use crate::hub::{EchoGuard, Hub, RateLimiter};
use crate::protocol::{now_ms, DriveLevel, ServerMsg};

/// Cap on the server-side rolling buffer of recent `affect` snapshots kept in
/// `AppState::affect_history` (see `lib.rs`) and served by
/// `GET /api/affect/history`.
///
/// Matches the browser's own `MAX_AFFECT_HISTORY` (useNpcSocket.ts) exactly:
/// the 感情 tab's sparkline never plots more points than that, so holding
/// more here would just be memory the trend line can never use. It also
/// isn't smaller — the whole reason this buffer exists is so a freshly
/// loaded page can seed the sparkline as deep as a tab that's been open a
/// while already shows, and a smaller cap would shortchange exactly that
/// case. Being a fixed-size ring (oldest entry evicted on overflow, see
/// `push_capped`) is what keeps this bounded over a server run measured in
/// days rather than turns, with nothing further to watch for.
pub(crate) const AFFECT_HISTORY_CAP: usize = 60;

/// One `affect` snapshot as kept in the server-side rolling buffer.
///
/// Field names/casing intentionally mirror `ServerMsg::Affect` exactly (same
/// `#[serde(rename = ...)]` choices), so `GET /api/affect/history`'s JSON is
/// byte-for-byte the shape of a live `affect` WS frame's payload (minus the
/// `"type"` tag) and the browser can decode both with its existing
/// `AffectSnapshot` type (useNpcSocket.ts) rather than needing a second one.
#[derive(Debug, Clone, serde::Serialize)]
pub(crate) struct AffectHistoryEntry {
    pub ts: i64,
    pub familiarity: f32,
    pub closing: bool,
    #[serde(rename = "inviteCaution")]
    pub invite_caution: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub partner: Option<String>,
    #[serde(rename = "partnerKnown")]
    pub partner_known: bool,
    #[serde(rename = "partnerSwitched")]
    pub partner_switched: bool,
    #[serde(rename = "partnerAway")]
    pub partner_away: bool,
    pub drives: Vec<DriveLevel>,
}

/// Append `entry` to `history`, evicting the single oldest entry first if
/// that would push it past `cap`. Oldest-first ordering is an invariant this
/// function maintains on every call (not just something incidentally true at
/// the cap) — `GET /api/affect/history` trusts that and never re-sorts.
/// `while` rather than `if` on the eviction check so the invariant holds even
/// if `history` somehow arrived already over `cap` (defensive; the only
/// production caller only ever adds one entry at a time).
fn push_capped(history: &mut VecDeque<AffectHistoryEntry>, entry: AffectHistoryEntry, cap: usize) {
    if cap == 0 {
        return;
    }
    while history.len() >= cap {
        history.pop_front();
    }
    history.push_back(entry);
}

/// Topic used by other modules (npc-action, npc-vision, npc-speech,
/// npc-translate, ...) to
/// push UI-only updates that don't fit the existing `agent:*` topics (e.g.
/// avatar position, current TTS playback line, a raw action log line). Not
/// part of `npc_core::bus::topic` since it's consumed only by npc-server.
const UI_TOPIC: &str = "npc:ui";

pub fn spawn(
    ctx: ModuleCtx,
    hub: Hub,
    echo: std::sync::Arc<EchoGuard>,
    short_term_memory: Arc<Mutex<String>>,
    affect_history: Arc<Mutex<VecDeque<AffectHistoryEntry>>>,
) {
    let mut rx = ctx.bus.subscribe();
    let shutdown = ctx.shutdown.clone();
    let volume_limiter = RateLimiter::per_second(10);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = shutdown.cancelled() => break,
                received = rx.recv() => {
                    match received {
                        Ok(bus_msg) => handle(&hub, &echo, &volume_limiter, &short_term_memory, &affect_history, bus_msg),
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
    affect_history: &Arc<Mutex<VecDeque<AffectHistoryEntry>>>,
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
                // No `ttsLine` broadcast here anymore: npc-speech now
                // publishes `npc:ui`/`tts_line` per sentence as it actually
                // speaks (see the `"tts_line"` arm in `handle_ui` below),
                // which is the only producer of that frame from here on.
                // Broadcasting the whole reply here too would show the UI
                // every line twice — once from this arm, once per sentence
                // from npc-speech — and `ttsLine` is now specifically "a line
                // being read aloud", so it is correctly absent when TTS is
                // off (whereas this arm fires on every `chat_response`
                // regardless of whether anything gets spoken).
                hub.broadcast(&ServerMsg::Response {
                    // `request_id` rides on the `chat_response` payload as a
                    // convention (see `ws::speech_payload`): npc-talk copies
                    // it through from the `agent:sense`/`speech` payload that
                    // triggered this turn. Absent means the NPC started this
                    // turn on its own initiative — a scheduler announcement,
                    // a vision-triggered remark — which genuinely has no
                    // requester to correlate to; inventing an id for it would
                    // claim a correlation that doesn't exist, so "-" (no
                    // request in flight) is kept instead.
                    request_id: request_id_field(&env.payload).unwrap_or_else(|| "-".to_string()),
                    status: "done".to_string(),
                    text: Some(content),
                    message: None,
                });
            } else if env.r#type == msg::CHAT_SILENT {
                // The turn was deliberately left unanswered (npc-talk's
                // `publish_silence`). No `chat`/`ttsLine` frame — nothing was
                // said — but the client still needs to know the turn is over,
                // otherwise its typing indicator spins forever.
                let reason = env
                    .payload
                    .get("reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_string();
                hub.broadcast(&ServerMsg::Silent { reason, ts: now_ms() });
            } else if env.r#type == msg::CHAT_ERROR {
                // npc-talk's turn failed outright (the LLM call errored, a
                // tool call blew up), so neither `chat_response` nor
                // `chat_silent` is coming — see
                // `ChatEngine::chat_with_context`, which wraps the whole turn
                // so every failure path reports itself. Without this the
                // client that sent the `input` behind that turn would wait on
                // a `response` frame that never arrives.
                //
                // npc-talk only publishes this when the turn actually had a
                // requester, so unlike the arms above there is no "-"
                // fallback to consider: an error with nobody waiting on it is
                // a log line, not a frame.
                if let Some(request_id) = request_id_field(&env.payload) {
                    let message = env
                        .payload
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("chat turn failed")
                        .to_string();
                    hub.broadcast(&ServerMsg::Response {
                        request_id,
                        status: "error".to_string(),
                        text: None,
                        message: Some(message),
                    });
                }
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
        UI_TOPIC => handle_ui(hub, volume_limiter, affect_history, env.r#type.as_str(), &env.payload),
        // `topic::CONFIG` ("npc:config") is deliberately NOT matched here: its
        // payload is the full, unredacted Config (including api_key secrets)
        // and must never reach a WS client. Only topics explicitly handled
        // above are forwarded — this match is a whitelist, not a blocklist.
        _ => {}
    }
}

fn handle_ui(
    hub: &Hub,
    volume_limiter: &RateLimiter,
    affect_history: &Arc<Mutex<VecDeque<AffectHistoryEntry>>>,
    msg_type: &str,
    payload: &Value,
) {
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
        // Not rate-limited like `volume`: npc-speech only publishes this on a
        // real start/stop transition, so the traffic is a couple of frames
        // per reply — and dropping one would leave the avatar's mouth stuck
        // open (or shut) until the next utterance.
        "speaking" => {
            if let Some(active) = payload.get("active").and_then(Value::as_bool) {
                hub.broadcast(&ServerMsg::Speaking { active });
            }
        }
        // Also not rate-limited here, but for a different reason than
        // `speaking` above: npc-speech's playback thread already throttles
        // this to roughly once per 50ms itself (see
        // `playback::LEVEL_PUBLISH_INTERVAL`), so a second limiter on this
        // side would only add complexity. Deliberately NOT sharing
        // `volume_limiter` with the mic `"volume"` arm either way — two
        // unrelated sources drawing from one rate budget would have each
        // starve the other under load.
        "speaking_level" => {
            if let Some(level) = payload.get("level").and_then(Value::as_f64) {
                hub.broadcast(&ServerMsg::SpeakingLevel { level });
            }
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
                    // One shared `ts`/`drives` for both the buffered copy and
                    // the live broadcast below: they describe the same
                    // instant, and giving them the same `ts` is exactly what
                    // lets the browser recognize (by `ts`) that a snapshot it
                    // already has live is the same one showing up in a
                    // `GET /api/affect/history` backfill, instead of
                    // double-counting it (see BrainView.tsx).
                    let ts = now_ms();
                    let drives: Vec<DriveLevel> = p
                        .drives
                        .into_iter()
                        .map(|d| DriveLevel {
                            key: d.key,
                            level: d.level,
                            base: d.base,
                        })
                        .collect();

                    // Buffered BEFORE the broadcast so a REST request that
                    // races this WS frame can never observe the buffer
                    // lagging one snapshot behind what clients were just
                    // sent live.
                    push_capped(
                        &mut affect_history.lock().unwrap(),
                        AffectHistoryEntry {
                            ts,
                            familiarity: p.familiarity,
                            closing: p.closing,
                            invite_caution: p.invite_caution,
                            partner: p.partner.clone(),
                            partner_known: p.partner_known,
                            partner_switched: p.partner_switched,
                            partner_away: p.partner_away,
                            drives: drives.clone(),
                        },
                        AFFECT_HISTORY_CAP,
                    );

                    hub.broadcast(&ServerMsg::Affect {
                        ts,
                        familiarity: p.familiarity,
                        closing: p.closing,
                        invite_caution: p.invite_caution,
                        partner: p.partner,
                        partner_known: p.partner_known,
                        partner_switched: p.partner_switched,
                        partner_away: p.partner_away,
                        drives,
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

/// Pull the optional `request_id` a `chat_response`/`chat_silent`/
/// `chat_error` payload was tagged with (see `ws::speech_payload`'s doc
/// comment for where that tag originates and why it's a snake_case
/// convention on the JSON rather than a new bus message shape).
fn request_id_field(payload: &Value) -> Option<String> {
    payload.get("request_id").and_then(Value::as_str).map(|s| s.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_field_prefers_content_over_text() {
        let payload = serde_json::json!({ "content": "c", "text": "t" });
        assert_eq!(text_field(&payload), Some("c".to_string()));
    }

    #[test]
    fn text_field_falls_back_to_text_when_content_absent() {
        let payload = serde_json::json!({ "text": "t" });
        assert_eq!(text_field(&payload), Some("t".to_string()));
    }

    #[test]
    fn request_id_field_reads_the_tagged_id() {
        let payload = serde_json::json!({ "content": "c", "request_id": "req-1" });
        assert_eq!(request_id_field(&payload), Some("req-1".to_string()));
    }

    /// A turn npc-talk started on its own (a scheduler announcement, a
    /// vision-triggered remark) has no `request_id` on its payload at all —
    /// this is the case `handle`'s `chat_response` arm falls back to `"-"`
    /// for, so the absence itself has to come back as `None`, not e.g. an
    /// empty string that would need a second check downstream.
    #[test]
    fn request_id_field_absent_is_none() {
        let payload = serde_json::json!({ "content": "c" });
        assert_eq!(request_id_field(&payload), None);
    }

    /// A minimal, otherwise-blank entry — only `ts` varies across the tests
    /// below, since that's all `push_capped` looks at.
    fn entry(ts: i64) -> AffectHistoryEntry {
        AffectHistoryEntry {
            ts,
            familiarity: 0.0,
            closing: false,
            invite_caution: false,
            partner: None,
            partner_known: false,
            partner_switched: false,
            partner_away: false,
            drives: Vec::new(),
        }
    }

    #[test]
    fn push_capped_keeps_oldest_first_order_under_cap() {
        let mut history = VecDeque::new();
        push_capped(&mut history, entry(1), 5);
        push_capped(&mut history, entry(2), 5);
        push_capped(&mut history, entry(3), 5);
        assert_eq!(history.iter().map(|e| e.ts).collect::<Vec<_>>(), vec![1, 2, 3]);
    }

    /// Past the cap, the *oldest* entry is the one that goes — a naive
    /// "drop the newest" bug would silently discard exactly the live turn
    /// that just happened, which is the one thing this buffer must not lose.
    #[test]
    fn push_capped_evicts_oldest_entry_when_over_cap() {
        let mut history = VecDeque::new();
        for ts in 1..=5 {
            push_capped(&mut history, entry(ts), 3);
        }
        assert_eq!(history.iter().map(|e| e.ts).collect::<Vec<_>>(), vec![3, 4, 5]);
    }

    /// `cap: 0` is a degenerate but reachable input (e.g. a future config
    /// knob) and must not panic on the `pop_front` of an empty deque.
    #[test]
    fn push_capped_with_zero_cap_is_a_no_op() {
        let mut history = VecDeque::new();
        push_capped(&mut history, entry(1), 0);
        assert!(history.is_empty());
    }

    /// A `history` that somehow already exceeds `cap` (shouldn't happen via
    /// this crate's own calls, but the function is defensive about it) still
    /// converges to exactly `cap` oldest-first entries after one more push.
    #[test]
    fn push_capped_trims_a_history_already_over_cap() {
        let mut history: VecDeque<AffectHistoryEntry> = (1..=4).map(entry).collect();
        push_capped(&mut history, entry(5), 2);
        assert_eq!(history.iter().map(|e| e.ts).collect::<Vec<_>>(), vec![4, 5]);
    }
}
