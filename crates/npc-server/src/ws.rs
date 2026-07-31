//! `GET /ws` handler: upgrades to a WebSocket, registers the connection with
//! the [`Hub`], sends `hello`, then pumps client frames in and forwards
//! queued outgoing frames out until the socket closes or shutdown fires.

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::IntoResponse;
use futures_util::{SinkExt, StreamExt};
use npc_core::{msg, topic};
use std::collections::VecDeque;
use std::sync::atomic::Ordering;
use uuid::Uuid;

use crate::protocol::{now_ms, AvatarRef, CharacterRef, ClientMsg, ModuleFlags, ServerMsg};
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

fn character_ref(state: &AppState) -> Option<CharacterRef> {
    npc_core::active_character(&state.ctx.data_dir, &state.current_config())
        .ok()
        .flatten()
        .map(|c| CharacterRef {
            id: c.id,
            name: c.sheet.name,
        })
}

/// Which body the UI should display: the active character's own avatar if it
/// has one, otherwise the standalone `config.character.avatar_file`.
///
/// The fallback is what lets an avatar be used with no character sheet at
/// all — tc-npc chats fine without one, so an avatar shouldn't require
/// importing a tc-town export first. A name that isn't in the library it
/// belongs to is dropped here rather than sent for the browser to fail on.
///
/// **Which library that is depends on `Avatar::kind`**: a `sprite` is looked
/// up in the sheet folder, anything else in the VRM folder. The bare
/// `avatar_file` fallback stays a VRM, since that config field predates
/// there being a second kind and every existing value in it is a model.
pub fn avatar_ref(state: &AppState) -> Option<AvatarRef> {
    let config = state.current_config();
    let from_character = npc_core::active_character(&state.ctx.data_dir, &config)
        .ok()
        .flatten()
        .and_then(|c| c.avatar);

    let avatar = match from_character {
        Some(avatar) => avatar,
        None => {
            let file = config.character.avatar_file.trim();
            if file.is_empty() {
                return None;
            }
            npc_core::Avatar::vrm(file)
        }
    };

    let exists = match avatar.kind.as_str() {
        "sprite" => npc_core::sprite_exists(&state.ctx.data_dir, &avatar.file),
        // Anything else is a model. Treating an unrecognised kind as VRM
        // rather than rejecting it is deliberate: `Avatar::kind` exists so a
        // new kind can be added without changing the wire shape, and a
        // character written by a newer build should degrade to "the model
        // isn't there" rather than to a hard failure.
        _ => npc_core::vrm_exists(&state.ctx.data_dir, &avatar.file),
    };

    match exists {
        Ok(true) => Some(avatar.into()),
        _ => None,
    }
}

/// Built from the *current* config, not the startup snapshot: npc-speech
/// starts and stops with `stt`/`tts` while the app runs, so a tab opened
/// after that toggle has to see where the switch actually is.
fn build_hello(state: &AppState) -> ServerMsg {
    ServerMsg::Hello {
        version: "1".to_string(),
        modules: module_flags(&state.current_config()),
        character: character_ref(state),
        avatar: avatar_ref(state),
    }
}

/// Bound on how many suspended `input`/`event` frames npc-server holds while
/// waiting for `resume`.
///
/// `suspend` has no hard time limit at this layer: npc-speech applies its own
/// 600s (`SUSPEND_TIMEOUT`) auto-expiry to TTS *playback*, but the
/// input/event processing gate here stays shut until an explicit `resume` or
/// the suspending client disconnecting (see [`SuspendGate`] below) — an
/// operator could legitimately leave the NPC suspended well past that for an
/// afk break, and a live stream keeps producing chat/events the whole time.
/// Without a cap, that backlog would grow without bound for as long as the
/// pause lasts. 100 is enough slack for a normal pause (a burst of viewer
/// events, a few typed messages) without letting an extended or forgotten
/// suspend turn into unbounded memory growth.
const SUSPEND_QUEUE_CAP: usize = 100;

/// One `input`/`event` frame held back by [`SuspendGate`] because it arrived
/// while suspended, carrying everything [`process_input`]/[`process_event`]
/// need to handle it exactly as if it had just arrived live.
enum QueuedRequest {
    Input {
        client_id: u64,
        text: String,
        speaker: Option<String>,
    },
    Event {
        client_id: u64,
        kind: String,
        user_name: Option<String>,
        text: Option<String>,
        amount: Option<f64>,
        tier: Option<String>,
        message: Option<String>,
        reward_title: Option<String>,
    },
}

/// Server-side gate backing `suspend`/`resume`: tc-npc's bus-level
/// `suspend`/`resume` (see `msg::SUSPEND`/`msg::RESUME`) only pauses TTS
/// playback in npc-speech, which does not satisfy the extension API spec's
/// "入力・イベントの処理を一時停止する" — the NPC kept generating replies
/// while "suspended", it just didn't speak them. This gate additionally
/// holds incoming `input`/`event` frames here at the WS layer so nothing is
/// even sent into the talk pipeline until `resume`.
#[derive(Default)]
pub(crate) struct SuspendGate {
    suspended: bool,
    /// The client id whose `suspend` is currently in effect, so a disconnect
    /// can tell "the suspending tab left" apart from "an unrelated tab
    /// closed" (see `handle_socket`'s disconnect handling). `None` when not
    /// suspended. If a second client sends `suspend` while already
    /// suspended, this is overwritten to the new client — reasonable given
    /// there is only one gate to hold, and it keeps auto-resume tied to
    /// whichever client most recently asked for the pause.
    suspended_by: Option<u64>,
    queue: VecDeque<QueuedRequest>,
}

/// If the gate is currently shut, push `req` onto its bounded queue and
/// return `true` (the caller must not process `req` now — it will be
/// replayed by [`drain_suspend_queue`] on `resume`). Returns `false` when not
/// suspended, meaning the caller should handle `req` immediately as usual.
///
/// On overflow the *oldest* queued item is dropped, not the new one and not
/// the whole queue: losing what just happened (the newest arrival) would be
/// worse than losing the tail end of a backlog that's already stale by the
/// time it would be replayed, and silently growing without bound is exactly
/// what [`SUSPEND_QUEUE_CAP`] exists to prevent.
fn enqueue_if_suspended(state: &AppState, req: QueuedRequest) -> bool {
    let mut gate = state.suspend_gate.lock().unwrap();
    if !gate.suspended {
        return false;
    }
    if gate.queue.len() >= SUSPEND_QUEUE_CAP {
        tracing::warn!(
            cap = SUSPEND_QUEUE_CAP,
            "npc-server: suspended input/event queue full, dropping oldest queued frame"
        );
        gate.queue.pop_front();
    }
    gate.queue.push_back(req);
    true
}

/// Replay every frame queued while suspended, in the order it arrived, via
/// the same `process_input`/`process_event` path a live frame takes. Drains
/// under the lock but processes after releasing it, so a `process_*` call
/// (which itself touches the bus/hub, not `suspend_gate`) never runs while
/// holding it.
fn drain_suspend_queue(state: &AppState) {
    let items: Vec<QueuedRequest> = {
        let mut gate = state.suspend_gate.lock().unwrap();
        gate.queue.drain(..).collect()
    };
    for req in items {
        match req {
            QueuedRequest::Input { client_id, text, speaker } => process_input(state, client_id, text, speaker),
            QueuedRequest::Event {
                client_id,
                kind,
                user_name,
                text,
                amount,
                tier,
                message,
                reward_title,
            } => process_event(state, client_id, kind, user_name, text, amount, tier, message, reward_title),
        }
    }
}

/// Re-open the gate and work through anything that queued up while shut.
/// `ack_to`, when `Some`, gets the `resumeAccepted` frame — the explicit
/// `resume` path passes the client that asked; the disconnect-triggered
/// auto-resume (see `handle_socket`) passes `None` since the client that
/// would have received it is the one that just left.
///
/// The bus `resume` message is still published unconditionally, exactly as
/// before this gate existed: npc-speech's TTS-pause behavior (and its own
/// 600s auto-expiry) must keep working whether or not anything was queued at
/// the WS layer.
fn do_resume(state: &AppState, ack_to: Option<u64>) {
    {
        let mut gate = state.suspend_gate.lock().unwrap();
        gate.suspended = false;
        gate.suspended_by = None;
    }
    if let Some(client_id) = ack_to {
        state.hub.send_to(client_id, &ServerMsg::ResumeAccepted);
    }
    state
        .ctx
        .bus
        .publish(topic::INTERRUPT, msg::RESUME, serde_json::json!({}));
    drain_suspend_queue(state);
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sink, mut stream) = socket.split();
    let (client_id, mut rx) = state.hub.register();

    state.hub.send_to(client_id, &build_hello(&state));
    // Right after hello so the チャット tab's 音声 開始/停止 toggle renders in
    // the position the running agent is actually in, not a guessed default.
    state.hub.send_to(
        client_id,
        &ServerMsg::Voice {
            active: state.voice_active.load(Ordering::SeqCst),
        },
    );

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

    // Auto-resume if (and only if) this was the client that suspended: per
    // the extension API spec ("WS切断時はコアが自動でresumeする"), a crashed
    // or closed extension must not wedge the NPC suspended forever. But
    // tc-npc routinely has several browser tabs connected at once, so this
    // has to check *which* client is gone rather than resuming on any
    // disconnect — otherwise closing an unrelated tab would silently
    // un-suspend a session another still-open tab deliberately paused.
    let should_auto_resume = {
        let gate = state.suspend_gate.lock().unwrap();
        gate.suspended && gate.suspended_by == Some(client_id)
    };
    if should_auto_resume {
        tracing::info!(
            client_id,
            "npc-server: the client that suspended disconnected, auto-resuming"
        );
        do_resume(&state, None);
    }

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
            let req = QueuedRequest::Input {
                client_id,
                text: text.clone(),
                speaker: speaker.clone(),
            };
            if !enqueue_if_suspended(state, req) {
                process_input(state, client_id, text, speaker);
            }
        }
        ClientMsg::Command { text } => {
            state
                .ctx
                .bus
                .publish(topic::ACTION, "command", serde_json::json!({ "text": text }));
        }
        ClientMsg::Interrupt => {
            // Dedicated "interrupt" type on agent:interrupt, distinct from
            // suspend/resume: npc-speech stops the reply being spoken and
            // drops the rest of it, then goes straight back to being ready
            // for the next one — no sticky paused state and no timeout.
            // A module that doesn't recognize the type ignores it, matching
            // the bus's fire-and-forget semantics.
            //
            // Not gated by `suspend_gate`: `interrupt` cuts whatever is
            // playing *right now* and is valid even while suspended (see the
            // spec's `suspend` の説明 — 「`interrupt` は... `suspend` 中でも
            // 有効」), unlike `input`/`event` which start new work.
            state
                .ctx
                .bus
                .publish(topic::INTERRUPT, msg::INTERRUPT, serde_json::json!({}));
            state.hub.send_to(client_id, &ServerMsg::InterruptAccepted);
        }
        ClientMsg::Suspend => {
            {
                let mut gate = state.suspend_gate.lock().unwrap();
                gate.suspended = true;
                gate.suspended_by = Some(client_id);
            }
            state.hub.send_to(client_id, &ServerMsg::SuspendAccepted);
            state
                .ctx
                .bus
                .publish(topic::INTERRUPT, msg::SUSPEND, serde_json::json!({}));
        }
        ClientMsg::Resume => do_resume(state, Some(client_id)),
        ClientMsg::VoiceStart => set_voice_active(state, true),
        ClientMsg::VoiceStop => set_voice_active(state, false),
        ClientMsg::Event {
            kind,
            user_name,
            text,
            amount,
            tier,
            message,
            reward_title,
        } => {
            let req = QueuedRequest::Event {
                client_id,
                kind: kind.clone(),
                user_name: user_name.clone(),
                text: text.clone(),
                amount,
                tier: tier.clone(),
                message: message.clone(),
                reward_title: reward_title.clone(),
            };
            if !enqueue_if_suspended(state, req) {
                process_event(state, client_id, kind, user_name, text, amount, tier, message, reward_title);
            }
        }
    }
}

/// Handle one `input` frame: ack it, echo it into the transcript, and hand it
/// to the talk pipeline over the bus. Split out from `handle_client_message`
/// so [`drain_suspend_queue`] can replay a queued frame through exactly the
/// same path a live one takes — a queued `input` must look, downstream, like
/// nothing was ever paused.
fn process_input(state: &AppState, client_id: u64, text: String, speaker: Option<String>) {
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
    state.ctx.bus.publish(
        topic::SENSE,
        msg::SPEECH,
        // Not a stream event, so no `transliterate_name` — see
        // `speech_payload`'s doc for why that flag is event-only.
        speech_payload(&text, speaker.as_deref(), Some(&request_id), false),
    );
}

/// Handle one `event` frame: ack it, build the in-persona instruction line,
/// echo it, and hand it to the talk pipeline. See [`process_input`] for why
/// this is its own function rather than inline in the match arm.
#[allow(clippy::too_many_arguments)]
fn process_event(
    state: &AppState,
    client_id: u64,
    kind: String,
    user_name: Option<String>,
    text: Option<String>,
    amount: Option<f64>,
    tier: Option<String>,
    message: Option<String>,
    reward_title: Option<String>,
) {
    state.hub.send_to(client_id, &ServerMsg::EventAccepted { kind: kind.clone() });
    // A fresh id per event turn, same as `input`: an event is still a request
    // this client made (a viewer follow/sub/etc. it observed and forwarded),
    // even though there is no `eventAccepted`-carried requestId for the
    // extension to remember it by — see `bus_forward.rs`'s `Response`
    // handling for why that still beats hardcoding "-" here.
    let request_id = Uuid::new_v4().to_string();
    let line = build_event_instruction(
        &kind,
        user_name.as_deref(),
        text.as_deref(),
        amount,
        tier.as_deref(),
        message.as_deref(),
        reward_title.as_deref(),
    );
    state.echo.mark(&line);
    state.hub.broadcast(&ServerMsg::Chat {
        role: "user".to_string(),
        text: line.clone(),
        ts: now_ms(),
    });
    state.ctx.bus.publish(
        topic::SENSE,
        msg::SPEECH,
        // `transliterate_name: true` — a viewer's `user_name` here is
        // routinely an English/Latin handle (Twitch/YouTube/etc. accounts),
        // which TTS reads letter-by-letter rather than as a name. Ported from
        // tc-assistant2's `hasLatinLetters`/`convertNameToKatakana`
        // (`src/main.tsx`): npc-talk (which owns the LLM client this needs)
        // reads this flag and, when `user_name` has Latin letters, asks the
        // LLM to transliterate it into katakana and substitutes it into this
        // same instruction line before the turn runs — see
        // `npc_talk::module::maybe_transliterate_event_name`.
        speech_payload(&line, user_name.as_deref(), Some(&request_id), true),
    );
}

/// Flip the cascade voice loop on or off: tell npc-speech over the bus, then
/// broadcast the new position so every open tab's toggle agrees (including
/// the one that didn't click it).
///
/// The broadcast is unconditional rather than only-on-change: a client whose
/// click raced another tab's still gets a frame confirming where the switch
/// ended up, instead of being left showing its own optimistic guess.
fn set_voice_active(state: &AppState, active: bool) {
    state.voice_active.store(active, Ordering::SeqCst);
    state.ctx.bus.publish(
        topic::INTERRUPT,
        if active {
            msg::VOICE_START
        } else {
            msg::VOICE_STOP
        },
        serde_json::json!({}),
    );
    state.hub.broadcast(&ServerMsg::Voice { active });
}

/// Build the `agent:sense`/`speech` payload for `content`, attributing it to
/// `speaker` when given a non-empty name (see the person-memory contract
/// §4: `{"content": "...", "speaker": "..."}`) and carrying `request_id`
/// (snake_case, a sibling of `speaker`) when this content came from a
/// specific WS request rather than e.g. a scheduler announcement. A missing
/// speaker/request_id simply omits that field, so existing consumers that
/// don't know about either see no change from before those fields existed.
///
/// `request_id` is a convention on this JSON payload, not a new bus message
/// shape: npc-talk (see the extension API compatibility work) reads it back
/// off this same payload and threads it through to the `chat_response`/
/// `chat_silent` envelope it publishes, which is what lets
/// `bus_forward.rs` correlate a `response` frame to the `input`/`event` that
/// produced it instead of hardcoding `requestId: "-"` for every turn.
///
/// `transliterate_name`, another sibling flag, is only ever `true` from
/// `process_event`: it tells npc-talk that `speaker`, when present and
/// containing Latin letters, is a stream-event viewer handle worth running
/// through its LLM's katakana transliteration before the turn (ported from
/// tc-assistant2's `hasLatinLetters`/`convertNameToKatakana`) — not a plain
/// conversation partner's name from `process_input`, which is left alone.
/// Omitted (rather than emitted as `false`) on every other payload, matching
/// this function's existing omit-when-absent treatment of `speaker`/
/// `request_id` above.
fn speech_payload(content: &str, speaker: Option<&str>, request_id: Option<&str>, transliterate_name: bool) -> serde_json::Value {
    let mut payload = serde_json::json!({ "content": content });
    if let Some(speaker) = speaker.map(str::trim).filter(|s| !s.is_empty()) {
        payload["speaker"] = serde_json::json!(speaker);
    }
    if let Some(request_id) = request_id {
        payload["request_id"] = serde_json::json!(request_id);
    }
    if transliterate_name {
        payload["transliterate_name"] = serde_json::json!(true);
    }
    payload
}

/// Turn a viewer/platform event into a short Japanese instruction line fed to
/// the talk module as if a viewer had said it, so the character reacts
/// in-persona rather than the UI needing its own reaction templates.
///
/// The framing (stage direction + "don't just read the description aloud")
/// mirrors tc-assistant2's `buildStreamEventPrompt` (`src/main.tsx`): telling
/// the model "you are a streaming character, react in character" produces an
/// actual in-character reaction, where tc-npc's old plain 「短くお礼を言って
/// ください」 style instruction tended to get read back near-verbatim.
#[allow(clippy::too_many_arguments)]
fn build_event_instruction(
    kind: &str,
    user_name: Option<&str>,
    text: Option<&str>,
    amount: Option<f64>,
    tier: Option<&str>,
    message: Option<&str>,
    reward_title: Option<&str>,
) -> String {
    let user = user_name.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("誰か");
    let tier_label = tier.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("不明");
    // A trailing comment/user-input line, when the event carries one
    // (`resub`/`cheer`'s viewer comment, `points`' redemption input) —
    // omitted entirely rather than rendered as an empty 「コメント:」 when
    // there isn't one.
    let comment_line = |label: &str, m: Option<&str>| -> String {
        match m.map(str::trim).filter(|s| !s.is_empty()) {
            Some(m) => format!("{label}:「{m}」"),
            None => String::new(),
        }
    };

    let detail = match kind {
        "follow" => format!("{user}さんが新しくフォローしてくれました。"),
        "subscribe" | "sub" => format!("{user}さんが新しくサブスク（{tier_label}）してくれました。"),
        // Previously fell through to the generic branch (losing months/tier/
        // comment entirely); now carries what the spec's event table says a
        // `resub` actually has.
        "resub" => {
            let months = amount.map(|a| a.to_string()).unwrap_or_else(|| "?".to_string());
            format!(
                "{user}さんがサブスクを更新してくれました（継続{months}ヶ月・{tier_label}）。{}",
                comment_line("コメント", message)
            )
        }
        // Split out from the old donation bucket: a gift-sub count is not a
        // 円 amount, and rendering it as one ("3円のギフト") was simply wrong.
        "gift" => {
            let count = amount.map(|a| a.to_string()).unwrap_or_else(|| "?".to_string());
            format!("{user}さんがサブスク（{tier_label}）を {count} 件ギフトしてくれました。")
        }
        // Also split out from the donation bucket: bits are not yen either.
        "cheer" => {
            let bits = amount.map(|a| a.to_string()).unwrap_or_else(|| "?".to_string());
            format!("{user}さんが {bits} bits を投げてくれました。{}", comment_line("コメント", message))
        }
        // Previously dropped the viewer count entirely; now uses it.
        "raid" => {
            let viewers = amount.map(|a| a.to_string()).unwrap_or_else(|| "?".to_string());
            format!("{user}さんが {viewers} 人を連れてレイドしてくれました。")
        }
        // Previously fell through to the generic branch; now uses
        // rewardTitle/the user's redemption input instead of a bare "points"
        // description.
        "points" => {
            let reward = reward_title.map(str::trim).filter(|s| !s.is_empty()).unwrap_or("不明");
            format!(
                "{user}さんがチャンネルポイント「{reward}」を交換しました。{}",
                comment_line("入力", message)
            )
        }
        // The genuine money-amount case: superchats and generic donations,
        // where rendering `amount` as 円 is actually correct.
        "donate" | "superchat" => match amount {
            Some(a) if a > 0.0 => format!("{user}さんから{a}円の投げ銭がありました。"),
            _ => format!("{user}さんから投げ銭がありました。"),
        },
        // Unknown kind: fall back to the event's own human-readable `text`,
        // exactly as before.
        other => match text.map(str::trim).filter(|s| !s.is_empty()) {
            Some(t) => format!("{other}というイベントが発生しました: {t}。"),
            None => format!("{other}というイベントが発生しました。"),
        },
    };

    [
        "【配信イベント】".to_string(),
        detail,
        "あなたは配信キャラクターです。視聴者に向けて、短く自然なリアクション（一言〜二言）を返してください。イベントの説明文をそのまま読み上げず、キャラクターとして反応してください。".to_string(),
    ]
    .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn speech_payload_without_speaker_matches_legacy_shape() {
        let payload = speech_payload("こんにちは", None, None, false);
        assert_eq!(payload, serde_json::json!({ "content": "こんにちは" }));
    }

    #[test]
    fn speech_payload_with_blank_speaker_omits_the_field() {
        let payload = speech_payload("こんにちは", Some("   "), None, false);
        assert_eq!(payload, serde_json::json!({ "content": "こんにちは" }));
    }

    #[test]
    fn speech_payload_with_speaker_includes_it() {
        let payload = speech_payload("こんにちは", Some("太郎"), None, false);
        assert_eq!(payload, serde_json::json!({ "content": "こんにちは", "speaker": "太郎" }));
    }

    #[test]
    fn speech_payload_with_request_id_includes_it_alongside_speaker() {
        let payload = speech_payload("こんにちは", Some("太郎"), Some("req-1"), false);
        assert_eq!(
            payload,
            serde_json::json!({ "content": "こんにちは", "speaker": "太郎", "request_id": "req-1" })
        );
    }

    #[test]
    fn speech_payload_with_request_id_only_omits_speaker() {
        let payload = speech_payload("こんにちは", None, Some("req-1"), false);
        assert_eq!(payload, serde_json::json!({ "content": "こんにちは", "request_id": "req-1" }));
    }

    /// `process_event` is the only caller that ever passes `true` here — see
    /// `speech_payload`'s doc for why plain speech input never needs it.
    #[test]
    fn speech_payload_with_transliterate_name_includes_the_flag() {
        let payload = speech_payload("Bobさんが新しくフォローしてくれました。", Some("Bob"), Some("req-1"), true);
        assert_eq!(
            payload,
            serde_json::json!({
                "content": "Bobさんが新しくフォローしてくれました。",
                "speaker": "Bob",
                "request_id": "req-1",
                "transliterate_name": true,
            })
        );
    }

    #[test]
    fn speech_payload_without_transliterate_name_omits_the_flag_rather_than_emitting_false() {
        let payload = speech_payload("こんにちは", Some("太郎"), None, false);
        assert!(payload.get("transliterate_name").is_none());
    }

    /// Every `build_event_instruction` call below shares this shape: a
    /// `【配信イベント】` header, one `detail` line describing what happened,
    /// then the in-character stage direction. Tests check the detail
    /// substring rather than the whole string so they don't have to repeat
    /// (and re-break-on-edit) that shared boilerplate.
    fn detail_line(kind: &str, user_name: Option<&str>, text: Option<&str>, amount: Option<f64>, tier: Option<&str>, message: Option<&str>, reward_title: Option<&str>) -> String {
        let line = build_event_instruction(kind, user_name, text, amount, tier, message, reward_title);
        line.lines().nth(1).unwrap_or_default().to_string()
    }

    #[test]
    fn build_event_instruction_uses_default_name_when_missing() {
        let detail = detail_line("follow", None, None, None, None, None, None);
        assert!(detail.starts_with("誰かさんが"), "{detail}");
    }

    #[test]
    fn build_event_instruction_wraps_detail_in_the_stage_direction_framing() {
        let line = build_event_instruction("follow", Some("太郎"), None, None, None, None, None);
        assert!(line.starts_with("【配信イベント】\n"), "{line}");
        assert!(
            line.contains("イベントの説明文をそのまま読み上げず、キャラクターとして反応してください"),
            "{line}"
        );
    }

    #[test]
    fn build_event_instruction_follow() {
        let detail = detail_line("follow", Some("太郎"), None, None, None, None, None);
        assert_eq!(detail, "太郎さんが新しくフォローしてくれました。");
    }

    #[test]
    fn build_event_instruction_subscribe_uses_tier() {
        let detail = detail_line("subscribe", Some("太郎"), None, None, Some("Tier 2"), None, None);
        assert_eq!(detail, "太郎さんが新しくサブスク（Tier 2）してくれました。");
    }

    #[test]
    fn build_event_instruction_resub_carries_months_tier_and_comment() {
        let detail = detail_line(
            "resub",
            Some("太郎"),
            None,
            Some(6.0),
            Some("Tier 1"),
            Some("いつも見てます"),
            None,
        );
        assert_eq!(
            detail,
            "太郎さんがサブスクを更新してくれました（継続6ヶ月・Tier 1）。コメント:「いつも見てます」"
        );
    }

    #[test]
    fn build_event_instruction_resub_without_comment_omits_comment_line() {
        let detail = detail_line("resub", Some("太郎"), None, Some(3.0), Some("Tier 1"), None, None);
        assert_eq!(detail, "太郎さんがサブスクを更新してくれました（継続3ヶ月・Tier 1）。");
    }

    /// `gift` (gift-sub count) must not be rendered as a 円 amount — that was
    /// the bug this task closes.
    #[test]
    fn build_event_instruction_gift_uses_sub_count_not_yen() {
        let detail = detail_line("gift", Some("太郎"), None, Some(5.0), Some("Tier 1"), None, None);
        assert_eq!(detail, "太郎さんがサブスク（Tier 1）を 5 件ギフトしてくれました。");
        assert!(!detail.contains('円'), "{detail}");
    }

    /// `cheer` (bits) must likewise not be rendered as a 円 amount.
    #[test]
    fn build_event_instruction_cheer_uses_bits_not_yen() {
        let detail = detail_line("cheer", Some("太郎"), None, Some(300.0), None, Some("応援してます"), None);
        assert_eq!(detail, "太郎さんが 300 bits を投げてくれました。コメント:「応援してます」");
        assert!(!detail.contains('円'), "{detail}");
    }

    #[test]
    fn build_event_instruction_raid_uses_viewer_count() {
        let detail = detail_line("raid", Some("太郎"), None, Some(42.0), None, None, None);
        assert_eq!(detail, "太郎さんが 42 人を連れてレイドしてくれました。");
    }

    #[test]
    fn build_event_instruction_points_uses_reward_title_and_input() {
        let detail = detail_line(
            "points",
            Some("太郎"),
            None,
            None,
            None,
            Some("こんにちは"),
            Some("たまごっちを見せて"),
        );
        assert_eq!(
            detail,
            "太郎さんがチャンネルポイント「たまごっちを見せて」を交換しました。入力:「こんにちは」"
        );
    }

    #[test]
    fn build_event_instruction_donate_still_renders_amount_as_yen() {
        let detail = detail_line("donate", Some("太郎"), None, Some(500.0), None, None, None);
        assert_eq!(detail, "太郎さんから500円の投げ銭がありました。");
    }

    #[test]
    fn build_event_instruction_unknown_kind_falls_back_to_text() {
        let detail = detail_line("raidshield", Some("太郎"), Some("何かが起きました"), None, None, None, None);
        assert_eq!(detail, "raidshieldというイベントが発生しました: 何かが起きました。");
    }

    #[test]
    fn build_event_instruction_unknown_kind_without_text_uses_bare_description() {
        let detail = detail_line("raidshield", Some("太郎"), None, None, None, None, None);
        assert_eq!(detail, "raidshieldというイベントが発生しました。");
    }
}
