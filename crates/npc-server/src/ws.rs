//! `GET /ws` handler: upgrades to a WebSocket, registers the connection with
//! the [`Hub`], sends `hello`, then pumps client frames in and forwards
//! queued outgoing frames out until the socket closes or shutdown fires.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use npc_core::{msg, topic};
use uuid::Uuid;

use crate::protocol::{now_ms, CharacterRef, ClientMsg, ModuleFlags, ServerMsg};
use crate::AppState;

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> impl IntoResponse {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

pub fn module_flags(config: &npc_core::Config) -> ModuleFlags {
    ModuleFlags {
        talk: config.talk.enabled,
        memory: config.memory.enabled,
        speech: config.tts.enabled || config.stt.enabled,
        vision: config.vision.enabled,
        action: config.action.enabled,
        scheduler: config.scheduler.enabled,
        translation: !config.translation.mode().is_off(),
    }
}

fn character_ref(ctx: &npc_core::ModuleCtx) -> Option<CharacterRef> {
    npc_core::active_character(&ctx.data_dir, &ctx.config)
        .ok()
        .flatten()
        .map(|c| CharacterRef {
            id: c.id,
            name: c.sheet.name,
        })
}

fn build_hello(ctx: &npc_core::ModuleCtx) -> ServerMsg {
    ServerMsg::Hello {
        version: "1".to_string(),
        modules: module_flags(&ctx.config),
        character: character_ref(ctx),
    }
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let (client_id, mut rx) = state.hub.register();

    state.hub.send_to(client_id, &build_hello(&state.ctx));

    let shutdown = state.ctx.shutdown.clone();
    let writer_shutdown = shutdown.clone();
    let writer = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = writer_shutdown.cancelled() => break,
                outgoing = rx.recv() => {
                    match outgoing {
                        Some(m) => {
                            if sink.send(m).await.is_err() {
                                break;
                            }
                        }
                        None => break,
                    }
                }
            }
        }
        let _ = sink.send(Message::Close(None)).await;
    });

    loop {
        tokio::select! {
            _ = shutdown.cancelled() => break,
            incoming = stream.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        handle_client_message(&state, client_id, &text).await;
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(err)) => {
                        tracing::debug!(error = %err, "npc-server: ws read error, closing");
                        break;
                    }
                }
            }
        }
    }

    state.hub.unregister(client_id);
    writer.abort();
}

async fn handle_client_message(state: &AppState, client_id: u64, text: &str) {
    let parsed: Result<ClientMsg, _> = serde_json::from_str(text);
    let client_msg = match parsed {
        Ok(m) => m,
        Err(err) => {
            state.hub.send_to(
                client_id,
                &ServerMsg::Error {
                    message: format!("invalid frame: {err}"),
                },
            );
            return;
        }
    };

    match client_msg {
        ClientMsg::Input { text, speaker } => {
            let request_id = Uuid::new_v4().to_string();
            state.hub.send_to(
                client_id,
                &ServerMsg::InputAccepted {
                    request_id: request_id.clone(),
                },
            );
            state.echo.mark(&text);
            state.hub.broadcast(&ServerMsg::Chat {
                role: "user".to_string(),
                text: text.clone(),
                ts: now_ms(),
            });
            state
                .ctx
                .bus
                .publish(topic::SENSE, msg::SPEECH, speech_payload(&text, speaker.as_deref()));
        }
        ClientMsg::Command { text } => {
            state
                .ctx
                .bus
                .publish(topic::ACTION, "command", serde_json::json!({ "text": text }));
        }
        ClientMsg::Interrupt => {
            // Dedicated "interrupt" type on agent:interrupt, distinct from
            // suspend/resume: modules that don't understand it yet (e.g. an
            // npc-speech that only knows suspend/resume/tts) can safely
            // ignore an unrecognized envelope type, matching the bus's
            // fire-and-forget semantics.
            state
                .ctx
                .bus
                .publish(topic::INTERRUPT, "interrupt", serde_json::json!({}));
        }
        ClientMsg::Suspend => {
            state
                .ctx
                .bus
                .publish(topic::INTERRUPT, msg::SUSPEND, serde_json::json!({}));
        }
        ClientMsg::Resume => {
            state
                .ctx
                .bus
                .publish(topic::INTERRUPT, msg::RESUME, serde_json::json!({}));
        }
        ClientMsg::Event {
            kind,
            user_name,
            text,
            amount,
        } => {
            let line = build_event_instruction(&kind, user_name.as_deref(), text.as_deref(), amount);
            state.echo.mark(&line);
            state.hub.broadcast(&ServerMsg::Chat {
                role: "user".to_string(),
                text: line.clone(),
                ts: now_ms(),
            });
            state.ctx.bus.publish(
                topic::SENSE,
                msg::SPEECH,
                speech_payload(&line, user_name.as_deref()),
            );
        }
    }
}

/// Build the `agent:sense`/`speech` payload for `content`, attributing it to
/// `speaker` when given a non-empty name (see the person-memory contract
/// §4: `{"content": "...", "speaker": "..."}`). A missing or blank speaker
/// leaves the payload exactly as before (`{"content": "..."}`) so existing
/// consumers that don't know about `speaker` see no change.
fn speech_payload(content: &str, speaker: Option<&str>) -> serde_json::Value {
    match speaker.map(str::trim).filter(|s| !s.is_empty()) {
        Some(speaker) => serde_json::json!({ "content": content, "speaker": speaker }),
        None => serde_json::json!({ "content": content }),
    }
}

/// Turn a viewer/platform event into a short Japanese instruction line fed
/// to the talk module as if a viewer had said it, so the character reacts
/// in-persona rather than the UI needing its own reaction templates.
fn build_event_instruction(kind: &str, user_name: Option<&str>, text: Option<&str>, amount: Option<f64>) -> String {
    let user = user_name.unwrap_or("誰か").to_string();
    match kind {
        "follow" => format!("{user}さんがフォローしてくれました。短くお礼を言ってください。"),
        "subscribe" | "sub" => format!("{user}さんがサブスクライブしてくれました。短くお礼を言ってください。"),
        "raid" => format!("{user}さんがレイドしてくれました。短く歓迎してください。"),
        "donate" | "gift" | "cheer" | "superchat" => match amount {
            Some(a) if a > 0.0 => format!("{user}さんから{a}円の投げ銭がありました。短くお礼を言ってください。"),
            _ => format!("{user}さんから投げ銭がありました。短くお礼を言ってください。"),
        },
        other => match text {
            Some(t) if !t.is_empty() => format!("{other}というイベントが発生しました: {t}。短く反応してください。"),
            _ => format!("{other}というイベントが発生しました。短く反応してください。"),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_payload_without_speaker_matches_legacy_shape() {
        let payload = speech_payload("こんにちは", None);
        assert_eq!(payload, serde_json::json!({ "content": "こんにちは" }));
    }

    #[test]
    fn speech_payload_with_blank_speaker_omits_the_field() {
        let payload = speech_payload("こんにちは", Some("   "));
        assert_eq!(payload, serde_json::json!({ "content": "こんにちは" }));
    }

    #[test]
    fn speech_payload_with_speaker_includes_it() {
        let payload = speech_payload("こんにちは", Some("太郎"));
        assert_eq!(payload, serde_json::json!({ "content": "こんにちは", "speaker": "太郎" }));
    }

    #[test]
    fn build_event_instruction_uses_default_name_when_missing() {
        let line = build_event_instruction("follow", None, None, None);
        assert!(line.starts_with("誰かさんが"));
    }
}
