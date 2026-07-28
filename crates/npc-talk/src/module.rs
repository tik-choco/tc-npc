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
        ));

        let mut rx = ctx.bus.subscribe();
        let mut state = TalkState {
            suspended: false,
            resume_deadline: None,
            interpret_mode: ctx.config.translation.suppresses_chat(),
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
                            handle_bus_message(&engine, &mut state, bus_msg);
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
}

fn handle_bus_message(engine: &Arc<ChatEngine>, state: &mut TalkState, bus_msg: BusMessage) {
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
                    if state.interpret_mode {
                        tracing::debug!(content = %content, "npc-talk: interpretation mode, leaving speech to npc-translate");
                    } else if state.suspended {
                        tracing::debug!(content = %content, "npc-talk: suspended, dropping speech input");
                    } else {
                        tracing::info!(content = %content, speaker = ?speaker, "npc-talk: received speech input");
                        let engine = engine.clone();
                        tokio::spawn(async move {
                            if let Err(err) = engine.chat_with_speaker(&content, speaker.as_deref()).await {
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
        }
    }

    #[test]
    fn config_update_toggles_interpretation_mode_both_ways() {
        let engine = engine();
        let mut state = state();

        let mut config = Config::default();
        config.translation.mode = "interpret".to_string();
        handle_bus_message(&engine, &mut state, config_updated(&config));
        assert!(state.interpret_mode);

        // `assist` keeps npc-talk answering — only `interpret` silences it.
        config.translation.mode = "assist".to_string();
        handle_bus_message(&engine, &mut state, config_updated(&config));
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

        handle_bus_message(&engine, &mut state, interrupt(msg::SUSPEND));
        assert!(state.suspended);
        assert!(state.resume_deadline.is_some());

        handle_bus_message(&engine, &mut state, interrupt(msg::RESUME));
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
        handle_bus_message(&engine, &mut state, msg);
        // Let the tokio::spawn'd `engine.set_person_memory` task actually run.
        tokio::task::yield_now().await;
    }
}
