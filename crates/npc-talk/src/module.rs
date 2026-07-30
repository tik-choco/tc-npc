//! The `Module` wiring: subscribes to the bus, dispatches `agent:mem` /
//! `agent:sense` / `agent:interrupt` messages to a [`ChatEngine`], and
//! respects shutdown. Ported from Go `agent-talk`'s `main.go` +
//! `HandleRedisMessage`, plus a suspend/resume feature that has no Go
//! equivalent (see task spec).
//!
//! One more thing can silence the engine: `config.translation.mode ==
//! "interpret"` (npc-translate's simultaneous-interpretation mode) means
//! heard speech is to be translated, not answered — so speech input is
//! dropped for as long as that mode is on. It's tracked from `npc:config`
//! updates rather than read once at startup so the 通訳 tab can switch modes
//! without a restart.
//!
//! The same `npc:config` subscription also carries the active character:
//! `config.character.active_id` is likewise re-read on every `CONFIG_UPDATED`
//! rather than only at startup, so switching characters in the web UI's
//! キャラ tab takes effect on the engine's very next turn instead of needing
//! the binary restarted. See [`apply_character_switch`].

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use npc_core::{msg, topic, BusMessage, Module, ModuleCtx};
use tokio::sync::broadcast::error::RecvError;
use tokio::time::Instant;

use crate::engine::ChatEngine;
use crate::tools::ToolRegistry;

/// Auto-resume timeout after a `suspend` with no matching `resume`.
const SUSPEND_TIMEOUT: Duration = Duration::from_secs(600);

/// How often to look for the conversation partner having gone quiet. Sets
/// the lag between the configured `absence_timeout_secs` elapsing and the UI
/// showing it, so it is much shorter than any sane timeout — but it is a
/// poll that runs forever, so not so short that it costs anything either.
const ABSENCE_CHECK_INTERVAL: Duration = Duration::from_secs(15);

pub struct TalkModule;

#[async_trait]
impl Module for TalkModule {
    fn name(&self) -> &'static str {
        "npc-talk"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        // Resolved once at startup (no hot-reload for the talk connection
        // itself, matching the pre-preset behavior).
        let resolved = ctx.config.resolve_llm(npc_core::LlmTask::Talk);
        let llm = npc_llm::LlmClient::new(resolved.base_url.clone(), resolved.api_key.clone())
            .with_reasoning_effort(resolved.reasoning_effort.clone());

        let active_character = npc_core::active_character(&ctx.data_dir, &ctx.config)
            .ok()
            .flatten();
        let persona = active_character.map(|c| npc_core::persona_prompt(&c.sheet));

        let engine = Arc::new(ChatEngine::new(
            llm,
            resolved.model,
            ctx.config.talk.prompts.clone(),
            ctx.config.talk.filter_prompt_name.clone(),
            ctx.config.talk.history_size as usize,
            ToolRegistry::new(),
            persona,
            &ctx.config.language,
            ctx.bus.clone(),
            ctx.config.talk.affect.clone(),
            ctx.config.talk.style_rules,
            ctx.config.talk.allow_silence,
        ));

        let mut rx = ctx.bus.subscribe();
        let mut state = TalkState {
            suspended: false,
            resume_deadline: None,
            interpret_mode: ctx.config.translation.suppresses_chat(),
            active_character_id: ctx.config.character.active_id.clone(),
        };

        // Noticing that the partner stopped talking needs a clock of its own:
        // no bus message arrives to announce silence. A fixed poll rather
        // than a deadline computed from the last turn, because the engine's
        // state sits behind an async mutex a chat turn may be holding — the
        // check is a cheap no-op whenever nothing has expired.
        let mut absence_check = tokio::time::interval(ABSENCE_CHECK_INTERVAL);
        absence_check.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

        loop {
            let sleep = async {
                match state.resume_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            };

            tokio::select! {
                _ = ctx.shutdown.cancelled() => {
                    tracing::info!("npc-talk: shutdown requested, stopping");
                    break;
                }
                _ = absence_check.tick() => {
                    let engine = engine.clone();
                    tokio::spawn(async move {
                        engine.check_partner_absence().await;
                    });
                }
                _ = sleep, if state.resume_deadline.is_some() => {
                    tracing::debug!("npc-talk: auto-resuming after suspend timeout");
                    state.suspended = false;
                    state.resume_deadline = None;
                }
                received = rx.recv() => {
                    match received {
                        Ok(bus_msg) => {
                            handle_bus_message(&engine, &mut state, bus_msg, &ctx.data_dir);
                        }
                        Err(RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "npc-talk: bus receiver lagged, messages were dropped");
                        }
                        Err(RecvError::Closed) => {
                            tracing::warn!("npc-talk: bus channel closed, stopping");
                            break;
                        }
                    }
                }
            }
        }

        Ok(())
    }
}

/// Everything that can gate a chat turn, kept together so the bus handler
/// takes one `&mut` instead of a growing parameter list.
struct TalkState {
    /// An `agent:interrupt` `suspend` is in effect.
    suspended: bool,
    /// When that suspend auto-expires (see [`SUSPEND_TIMEOUT`]).
    resume_deadline: Option<Instant>,
    /// `config.translation.mode == "interpret"` — heard speech belongs to
    /// npc-translate, not to a chat turn.
    interpret_mode: bool,
    /// `config.character.active_id` as last applied to the engine's
    /// persona. Compared against every incoming `CONFIG_UPDATED`'s value so
    /// the many config saves that have nothing to do with the character tab
    /// (every `PUT /api/config` publishes the same message) don't trigger a
    /// reload. Updated synchronously the moment a real change is detected —
    /// before the slower file read and engine update that follow it (see
    /// [`apply_character_switch`]) have even started — so a second change
    /// arriving while the first is still loading is judged against the
    /// right target instead of a now-stale one.
    active_character_id: String,
}

fn handle_bus_message(engine: &Arc<ChatEngine>, state: &mut TalkState, bus_msg: BusMessage, data_dir: &Path) {
    match bus_msg.topic.as_str() {
        topic::MEM => match bus_msg.env.r#type.as_str() {
            msg::SHORT_TERM_MEMORY => {
                if let Some(content) = extract_content(&bus_msg.env.payload) {
                    tracing::info!(content = %content, "npc-talk: updated short term memory");
                    let engine = engine.clone();
                    tokio::spawn(async move {
                        engine.set_short_term_memory(content).await;
                    });
                }
            }
            msg::LONG_TERM_MEMORY => {
                if let Some(content) = extract_content(&bus_msg.env.payload) {
                    tracing::info!(content = %content, "npc-talk: updated long term memory");
                    let engine = engine.clone();
                    tokio::spawn(async move {
                        engine.set_long_term_memory(content).await;
                    });
                }
            }
            msg::PERSON_MEMORY => {
                if let Some(content) = extract_content(&bus_msg.env.payload) {
                    tracing::info!(content = %content, "npc-talk: updated person memory");
                    let engine = engine.clone();
                    tokio::spawn(async move {
                        engine.set_person_memory(content).await;
                    });
                }
            }
            _ => {}
        },
        topic::SENSE => {
            if bus_msg.env.r#type == msg::SPEECH {
                if let Some(content) = extract_content(&bus_msg.env.payload) {
                    let speaker = extract_speaker(&bus_msg.env.payload);
                    // The WS API's per-request correlation id, if npc-server
                    // minted one for the `input` frame that produced this
                    // utterance (its `ws.rs` stamps it onto this same payload
                    // as `request_id`, a sibling of `speaker`). Absent for a
                    // self-initiated turn — a scheduler announcement, a vision
                    // remark — which is exactly what must reach the engine as
                    // `None` rather than an empty string (see
                    // `ChatEngine::chat_with_context`).
                    let request_id = extract_request_id(&bus_msg.env.payload);
                    if state.interpret_mode {
                        tracing::debug!(content = %content, "npc-talk: interpretation mode, leaving speech to npc-translate");
                    } else if state.suspended {
                        tracing::debug!(content = %content, "npc-talk: suspended, dropping speech input");
                    } else {
                        tracing::info!(content = %content, speaker = ?speaker, request_id = ?request_id, "npc-talk: received speech input");
                        let engine = engine.clone();
                        tokio::spawn(async move {
                            if let Err(err) = engine
                                .chat_with_context(&content, speaker.as_deref(), request_id.as_deref())
                                .await
                            {
                                tracing::error!(error = %err, "npc-talk: chat turn failed");
                            }
                        });
                    }
                }
            }
        }
        topic::INTERRUPT => match bus_msg.env.r#type.as_str() {
            msg::SUSPEND => {
                state.suspended = true;
                state.resume_deadline = Some(Instant::now() + SUSPEND_TIMEOUT);
                tracing::info!("npc-talk: suspended (auto-resume in 600s)");
            }
            msg::RESUME => {
                state.suspended = false;
                state.resume_deadline = None;
                tracing::info!("npc-talk: resumed");
            }
            _ => {}
        },
        topic::CONFIG => {
            if bus_msg.env.r#type == msg::CONFIG_UPDATED {
                match serde_json::from_value::<npc_core::Config>(bus_msg.env.payload) {
                    Ok(new_config) => {
                        let interpret_mode = new_config.translation.suppresses_chat();
                        if interpret_mode != state.interpret_mode {
                            tracing::info!(
                                interpret_mode,
                                "npc-talk: interpretation mode changed, speech input {}",
                                if interpret_mode { "now goes to npc-translate" } else { "answered again" }
                            );
                        }
                        state.interpret_mode = interpret_mode;

                        // `POST /api/characters/{id}/activate` publishes this
                        // same `CONFIG_UPDATED` message (see npc-server), but
                        // so does every unrelated `PUT /api/config` save —
                        // most of these carry `active_id` unchanged. Only
                        // react when it actually moved, both to skip a
                        // needless file read on the common case and so the
                        // log line in `apply_character_switch` means
                        // something when it does fire.
                        if new_config.character.active_id != state.active_character_id {
                            let old_id = std::mem::replace(
                                &mut state.active_character_id,
                                new_config.character.active_id.clone(),
                            );
                            let engine = engine.clone();
                            let data_dir = data_dir.to_path_buf();
                            tokio::spawn(async move {
                                apply_character_switch(&engine, &data_dir, &new_config, &old_id).await;
                            });
                        }
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "npc-talk: failed to deserialize updated config, keeping current mode");
                    }
                }
            }
        }
        _ => {}
    }
}

/// Load whatever character `new_config.character.active_id` now names and
/// install it as the engine's persona — or clear the persona entirely if the
/// id is empty or names a character that can't be loaded. Spawned by
/// [`handle_bus_message`] rather than run inline, because it does a file
/// read and takes the engine's per-turn lock (via [`ChatEngine::set_persona`]),
/// neither of which the bus-dispatch loop may block on.
///
/// Clearing on a load failure — rather than leaving `old_id`'s persona in
/// place — is deliberate: [`npc_core::active_character`] already collapses
/// "no such file" and "unreadable/corrupt" into a plain `Ok(None)`, and this
/// function has no way to tell those apart from "explicitly cleared" except
/// by checking whether `active_id` is empty. Either way, silently continuing
/// to role-play a character the operator just deactivated (or that vanished
/// from disk) is a worse failure mode than answering with no persona at all.
///
/// `old_id` is used only for the log line: the *decision* to switch (and the
/// bookkeeping in `TalkState::active_character_id`) already happened
/// synchronously in [`handle_bus_message`] before this task was spawned.
///
/// Deliberately touches nothing else in `ChatEngine`'s per-conversation
/// state: `history` (the running message list) and the per-partner
/// [`crate::affect::PartnerAffect`] drives both stay exactly as they were.
/// The alternative — wiping them, the way a full binary restart used to —
/// would make the "no more restart" feature swap in a fresh persona while
/// still throwing away the one thing a restart-free switch is supposed to
/// preserve: an ongoing conversation. A little residual confusion (the model
/// briefly seeing history from the outgoing persona) is a smaller cost than
/// that, and it self-corrects within a turn or two once the new persona
/// system message dominates generation. If a future persona ever needs a
/// harder reset (e.g. an operator-facing "also clear history" toggle), that
/// belongs in the server route that publishes the activation, not here —
/// this function only ever reacts to whatever `CONFIG_UPDATED` says.
async fn apply_character_switch(
    engine: &Arc<ChatEngine>,
    data_dir: &Path,
    new_config: &npc_core::Config,
    old_id: &str,
) {
    let character = npc_core::active_character(data_dir, new_config).ok().flatten();
    let from = if old_id.is_empty() { "(none)" } else { old_id };

    match &character {
        Some(c) => {
            tracing::info!(from, to = %c.sheet.name, "npc-talk: active character switched, persona reloaded");
        }
        None if new_config.character.active_id.is_empty() => {
            tracing::info!(from, "npc-talk: active character cleared, persona reset to none");
        }
        None => {
            tracing::warn!(
                from,
                active_id = %new_config.character.active_id,
                "npc-talk: active character could not be loaded, clearing persona instead of keeping the previous one",
            );
        }
    }

    let persona = character.map(|c| npc_core::persona_prompt(&c.sheet));
    engine.set_persona(persona).await;
}

/// Payload may be `{"content": "..."}` or a bare JSON string, matching Go's
/// fallback unmarshal in `HandleRedisMessage`.
fn extract_content(payload: &serde_json::Value) -> Option<String> {
    if let Some(s) = payload.as_str() {
        return Some(s.to_string());
    }
    payload.get("content").and_then(|v| v.as_str()).map(str::to_string)
}

/// Pull the optional `speaker` field off a `topic::SENSE`/`msg::SPEECH`
/// payload (person-memory contract §2). Absent when the payload is a bare
/// string or simply has no `speaker` key — both mean "speaker unknown".
fn extract_speaker(payload: &serde_json::Value) -> Option<String> {
    payload.get("speaker").and_then(|v| v.as_str()).map(str::to_string)
}

/// Pull the optional `request_id` field off a `topic::SENSE`/`msg::SPEECH`
/// payload — npc-server's `ws.rs` stamps the WS API's per-request id here (a
/// sibling of `speaker` above) when the utterance came from a client `input`
/// frame. Absent for a bare-string payload or one with no `request_id` key,
/// both of which mean "nobody is waiting on this turn" (a scheduler
/// announcement, a vision-triggered remark, or any other self-initiated
/// speech) rather than "the id is the empty string" — the distinction
/// `ChatEngine::chat_with_context` and npc-server's `bus_forward` both rely
/// on to decide whether a reply needs a `request_id` at all.
fn extract_request_id(payload: &serde_json::Value) -> Option<String> {
    payload.get("request_id").and_then(|v| v.as_str()).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;
    use npc_core::{Bus, Config, Envelope};

    fn engine() -> Arc<ChatEngine> {
        Arc::new(ChatEngine::new(
            npc_llm::LlmClient::new("http://localhost:0/v1", ""),
            "test-model".to_string(),
            Vec::new(),
            String::new(),
            10,
            ToolRegistry::new(),
            None,
            "auto",
            Bus::new(),
            npc_core::config::AffectConfig::default(),
            true,
            true,
        ))
    }

    fn config_updated(config: &Config) -> BusMessage {
        BusMessage {
            topic: topic::CONFIG.to_string(),
            env: Envelope {
                r#type: msg::CONFIG_UPDATED.to_string(),
                payload: serde_json::to_value(config).unwrap(),
            },
        }
    }

    fn state() -> TalkState {
        TalkState {
            suspended: false,
            resume_deadline: None,
            interpret_mode: false,
            active_character_id: String::new(),
        }
    }

    /// A `data_dir` for tests that don't exercise the character-reload path
    /// at all (nothing under it is ever read): `handle_bus_message` only
    /// touches its `data_dir` argument when `active_id` actually changes,
    /// and none of these do.
    fn unused_data_dir() -> std::path::PathBuf {
        std::env::temp_dir()
    }

    #[test]
    fn config_update_toggles_interpretation_mode_both_ways() {
        let engine = engine();
        let mut state = state();

        let mut config = Config::default();
        config.translation.mode = "interpret".to_string();
        handle_bus_message(&engine, &mut state, config_updated(&config), &unused_data_dir());
        assert!(state.interpret_mode);

        // `assist` keeps npc-talk answering — only `interpret` silences it.
        config.translation.mode = "assist".to_string();
        handle_bus_message(&engine, &mut state, config_updated(&config), &unused_data_dir());
        assert!(!state.interpret_mode);
    }

    #[test]
    fn broken_config_payload_keeps_the_current_mode() {
        let engine = engine();
        let mut state = state();
        state.interpret_mode = true;

        handle_bus_message(
            &engine,
            &mut state,
            BusMessage {
                topic: topic::CONFIG.to_string(),
                env: Envelope {
                    r#type: msg::CONFIG_UPDATED.to_string(),
                    payload: serde_json::json!("not a config"),
                },
            },
            &unused_data_dir(),
        );
        assert!(state.interpret_mode);
    }

    #[test]
    fn suspend_and_resume_still_gate_chat_turns() {
        let engine = engine();
        let mut state = state();

        let interrupt = |kind: &str| BusMessage {
            topic: topic::INTERRUPT.to_string(),
            env: Envelope {
                r#type: kind.to_string(),
                payload: serde_json::json!({}),
            },
        };

        handle_bus_message(&engine, &mut state, interrupt(msg::SUSPEND), &unused_data_dir());
        assert!(state.suspended);
        assert!(state.resume_deadline.is_some());

        handle_bus_message(&engine, &mut state, interrupt(msg::RESUME), &unused_data_dir());
        assert!(!state.suspended);
        assert!(state.resume_deadline.is_none());
    }

    #[test]
    fn extract_speaker_reads_speaker_field() {
        let payload = serde_json::json!({"content": "hi", "speaker": "太郎"});
        assert_eq!(extract_speaker(&payload), Some("太郎".to_string()));
    }

    #[test]
    fn extract_speaker_none_when_field_absent() {
        let payload = serde_json::json!({"content": "hi"});
        assert_eq!(extract_speaker(&payload), None);
    }

    #[test]
    fn extract_speaker_none_for_bare_string_payload() {
        // Matches `extract_content`'s bare-string fallback: a bare string has
        // no `speaker` field to read, so the speaker is unknown.
        let payload = serde_json::json!("hi");
        assert_eq!(extract_speaker(&payload), None);
    }

    #[test]
    fn extract_request_id_reads_request_id_field() {
        let payload = serde_json::json!({"content": "hi", "request_id": "req-123"});
        assert_eq!(extract_request_id(&payload), Some("req-123".to_string()));
    }

    #[test]
    fn extract_request_id_none_when_field_absent() {
        // A self-initiated turn (scheduler, vision) has no client waiting,
        // so npc-server never stamps a `request_id` onto its payload.
        let payload = serde_json::json!({"content": "hi"});
        assert_eq!(extract_request_id(&payload), None);
    }

    #[test]
    fn extract_request_id_none_for_bare_string_payload() {
        let payload = serde_json::json!("hi");
        assert_eq!(extract_request_id(&payload), None);
    }

    #[tokio::test]
    async fn person_memory_message_is_dispatched_without_panicking() {
        let engine = engine();
        let mut state = state();

        let msg = BusMessage {
            topic: topic::MEM.to_string(),
            env: Envelope {
                r#type: msg::PERSON_MEMORY.to_string(),
                payload: serde_json::json!({
                    "person_id": "abc",
                    "name": "太郎",
                    "content": "太郎について覚えていること:\n- 犬を飼っている",
                }),
            },
        };
        handle_bus_message(&engine, &mut state, msg, &unused_data_dir());
        // Let the tokio::spawn'd `engine.set_person_memory` task actually run.
        tokio::task::yield_now().await;
    }

    /// The "decision" half of the reload, isolated from the file-loading
    /// half: a changed `active_id` must update `TalkState`'s tracked id
    /// immediately (synchronously, before whatever `apply_character_switch`
    /// does), an unchanged one (the common case — most config saves have
    /// nothing to do with characters) must not spuriously re-trigger, and
    /// clearing `active_id` back to empty must be tracked just like any
    /// other change. Uses `#[tokio::test]` only because a real change makes
    /// `handle_bus_message` call `tokio::spawn`, which needs a runtime to
    /// exist — the assertions themselves are all synchronous.
    #[tokio::test]
    async fn active_character_id_changes_are_tracked_and_no_op_changes_are_not() {
        let engine = engine();
        let mut state = state();
        assert_eq!(state.active_character_id, "");

        let mut config = Config::default();
        config.character.active_id = "abc".to_string();
        handle_bus_message(&engine, &mut state, config_updated(&config), &unused_data_dir());
        assert_eq!(state.active_character_id, "abc");

        // Same id coming back around (an unrelated settings save) must read
        // as "nothing to do", not as a fresh switch.
        handle_bus_message(&engine, &mut state, config_updated(&config), &unused_data_dir());
        assert_eq!(state.active_character_id, "abc");

        config.character.active_id = String::new();
        handle_bus_message(&engine, &mut state, config_updated(&config), &unused_data_dir());
        assert_eq!(state.active_character_id, "");
    }

    /// End-to-end: a real character file on disk, activated then
    /// deactivated through the same `CONFIG_UPDATED` path npc-server uses,
    /// actually lands on (and later clears off) the engine — this is the
    /// restart this whole feature exists to remove. Polls `engine.persona()`
    /// for a few yields rather than asserting immediately, since the reload
    /// itself runs in a `tokio::spawn`'d task (a file read plus an
    /// uncontended lock — fast, but not guaranteed to have run yet the
    /// instant `handle_bus_message` returns).
    #[tokio::test]
    async fn character_switch_installs_the_new_persona_and_clearing_resets_it() {
        let data_dir = std::env::temp_dir().join(format!(
            "npc-talk-persona-switch-test-{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        let sheet = npc_core::CharacterSheet {
            name: "アリス".to_string(),
            summary: "元気な図書委員".to_string(),
            ..Default::default()
        };
        npc_core::save_character(
            &data_dir,
            &npc_core::Character {
                id: "alice".to_string(),
                created_at: String::new(),
                updated_at: String::new(),
                sheet: sheet.clone(),
                voice_model: None,
                voice_name: None,
                avatar: None,
            },
        )
        .unwrap();

        let engine = engine();
        let mut state = state();
        assert_eq!(engine.persona().await, None);

        let mut config = Config::default();
        config.character.active_id = "alice".to_string();
        handle_bus_message(&engine, &mut state, config_updated(&config), &data_dir);
        assert_eq!(state.active_character_id, "alice");

        let mut installed = engine.persona().await;
        for _ in 0..50 {
            if installed.is_some() {
                break;
            }
            tokio::task::yield_now().await;
            installed = engine.persona().await;
        }
        assert_eq!(installed, Some(npc_core::persona_prompt(&sheet)));

        // Deactivating (active_id back to empty) must clear the persona
        // rather than leave アリス's in place — the whole point of clearing
        // on anything but a clean load, spelled out on
        // `apply_character_switch`.
        config.character.active_id = String::new();
        handle_bus_message(&engine, &mut state, config_updated(&config), &data_dir);

        let mut cleared = engine.persona().await;
        for _ in 0..50 {
            if cleared.is_none() {
                break;
            }
            tokio::task::yield_now().await;
            cleared = engine.persona().await;
        }
        assert_eq!(cleared, None);

        let _ = std::fs::remove_dir_all(&data_dir);
    }
}
