//! The `Module` wiring: subscribes to the bus, dispatches `agent:mem` /
//! `agent:sense` / `agent:interrupt` messages to a [`ChatEngine`], and
//! respects shutdown. Ported from Go `agent-talk`'s `main.go` +
//! `HandleRedisMessage`, plus a suspend/resume feature that has no Go
//! equivalent (see task spec).

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

pub struct TalkModule;

#[async_trait]
impl Module for TalkModule {
    fn name(&self) -> &'static str {
        "npc-talk"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        let llm = npc_llm::LlmClient::new(ctx.config.api.base_url.clone(), ctx.config.api.api_key.clone());

        let active_character = npc_core::active_character(&ctx.data_dir, &ctx.config)
            .ok()
            .flatten();
        let persona = active_character.map(|c| npc_core::persona_prompt(&c.sheet));

        let reasoning_effort = if ctx.config.api.reasoning_effort.is_empty() {
            None
        } else {
            Some(ctx.config.api.reasoning_effort.clone())
        };

        let engine = Arc::new(ChatEngine::new(
            llm,
            ctx.config.api.model.clone(),
            reasoning_effort,
            ctx.config.talk.prompts.clone(),
            ctx.config.talk.filter_prompt_name.clone(),
            ctx.config.talk.history_size as usize,
            ToolRegistry::new(),
            persona,
            ctx.bus.clone(),
        ));

        let mut rx = ctx.bus.subscribe();
        let mut suspended = false;
        let mut resume_deadline: Option<Instant> = None;

        loop {
            let sleep = async {
                match resume_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending::<()>().await,
                }
            };

            tokio::select! {
                _ = ctx.shutdown.cancelled() => {
                    tracing::info!("npc-talk: shutdown requested, stopping");
                    break;
                }
                _ = sleep, if resume_deadline.is_some() => {
                    tracing::debug!("npc-talk: auto-resuming after suspend timeout");
                    suspended = false;
                    resume_deadline = None;
                }
                received = rx.recv() => {
                    match received {
                        Ok(bus_msg) => {
                            handle_bus_message(&engine, &mut suspended, &mut resume_deadline, bus_msg);
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

fn handle_bus_message(
    engine: &Arc<ChatEngine>,
    suspended: &mut bool,
    resume_deadline: &mut Option<Instant>,
    bus_msg: BusMessage,
) {
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
            _ => {}
        },
        topic::SENSE => {
            if bus_msg.env.r#type == msg::SPEECH {
                if let Some(content) = extract_content(&bus_msg.env.payload) {
                    if *suspended {
                        tracing::debug!(content = %content, "npc-talk: suspended, dropping speech input");
                    } else {
                        tracing::info!(content = %content, "npc-talk: received speech input");
                        let engine = engine.clone();
                        tokio::spawn(async move {
                            if let Err(err) = engine.chat(&content).await {
                                tracing::error!(error = %err, "npc-talk: chat turn failed");
                            }
                        });
                    }
                }
            }
        }
        topic::INTERRUPT => match bus_msg.env.r#type.as_str() {
            msg::SUSPEND => {
                *suspended = true;
                *resume_deadline = Some(Instant::now() + SUSPEND_TIMEOUT);
                tracing::info!("npc-talk: suspended (auto-resume in 600s)");
            }
            msg::RESUME => {
                *suspended = false;
                *resume_deadline = None;
                tracing::info!("npc-talk: resumed");
            }
            _ => {}
        },
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
