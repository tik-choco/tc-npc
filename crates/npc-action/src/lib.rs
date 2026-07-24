//! Action module (ports Go `agent-action`): drives a VRChat avatar over OSC
//! (`internal/osc/*.go`), tracks a dead-reckoned position (`Navigator`) and
//! higher-level go-to/route behavior (`Autopilot`), dispatches CLI-style
//! text commands (`internal/cli/*.go`), and turns free-form natural
//! language into a sequence of those commands via an LLM
//! (`internal/agent/agent.go`'s `ChatWithAction`).
//!
//! Bus wiring: subscribes `agent:action`. A `type: "action"` message (Go's
//! Redis `agent.MsgTypeAction`) — payload either a bare JSON string or
//! `{"content": "..."}` — is routed to the NL action-chat flow. A `type:
//! "command"` message — payload `{"text": "..."}` — is routed straight to
//! the command dispatcher. Every log line and position update is published
//! on `npc:ui` (`action_log` / `position`) for `npc-server` to forward to
//! the web UI over WebSocket.

mod autopilot;
mod controller;
mod dispatcher;
mod llm_action;
mod navigator;
mod osc;
mod state;
mod types;

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use npc_core::{msg, topic, Envelope, Module, ModuleCtx};
use npc_llm::{ChatMessage, LlmClient};
use serde_json::Value;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::Mutex as AsyncMutex;

use autopilot::Autopilot;
use controller::Controller;
use navigator::Navigator;
use osc::VrcClient;
use state::ActionState;

/// Bus message `type` for a plain command dispatch (`{"text": "..."}`).
/// Not among `npc_core::bus::msg`'s constants (which mirror only the Go
/// suite's cross-service message types), so it's a local literal here —
/// npc-server is expected to publish exactly this string on `agent:action`.
const MSG_TYPE_COMMAND: &str = "command";

struct ActionModule;

pub fn module(_ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>> {
    Ok(Box::new(ActionModule))
}

#[async_trait]
impl Module for ActionModule {
    fn name(&self) -> &'static str {
        "npc-action"
    }

    async fn run(self: Box<Self>, ctx: ModuleCtx) -> anyhow::Result<()> {
        let config = ctx.config.clone();

        let vrc = Arc::new(VrcClient::connect(&config.action.osc_address).await?);
        let controller = Controller::new(vrc.clone());
        let navigator = Arc::new(Navigator::new(vrc.clone(), ctx.bus.clone()));
        let autopilot = Autopilot::new(navigator.clone());
        let llm = LlmClient::new(config.api.base_url.clone(), config.api.api_key.clone());
        let static_system_prompt = llm_action::build_static_system_prompt(&config);

        let state = Arc::new(ActionState {
            bus: ctx.bus.clone(),
            config: config.clone(),
            shutdown: ctx.shutdown.clone(),
            vrc,
            controller,
            navigator,
            autopilot,
            llm,
            static_system_prompt: static_system_prompt.clone(),
            chat_history: AsyncMutex::new(vec![ChatMessage::system(static_system_prompt)]),
            chat_busy: AtomicBool::new(false),
            active: Mutex::new(None),
        });

        tracing::info!(addr = %config.action.osc_address, "npc-action: OSC client ready");

        let mut rx = ctx.bus.subscribe();
        loop {
            tokio::select! {
                _ = ctx.shutdown.cancelled() => {
                    dispatcher::stop_active_task(&state).await;
                    break;
                }
                msg = rx.recv() => {
                    match msg {
                        Ok(bus_msg) => {
                            if bus_msg.topic != topic::ACTION {
                                continue;
                            }
                            handle_bus_message(state.clone(), bus_msg.env);
                        }
                        Err(RecvError::Lagged(skipped)) => {
                            tracing::warn!(skipped, "npc-action: bus receiver lagged, some messages dropped");
                        }
                        Err(RecvError::Closed) => break,
                    }
                }
            }
        }

        Ok(())
    }
}

fn handle_bus_message(state: Arc<ActionState>, env: Envelope) {
    match env.r#type.as_str() {
        t if t == msg::ACTION => {
            let Some(text) = extract_text(&env.payload) else {
                tracing::warn!("npc-action: 'action' message missing text/content payload");
                return;
            };
            tokio::spawn(async move {
                llm_action::chat_with_action(state, text).await;
            });
        }
        MSG_TYPE_COMMAND => {
            let Some(text) = env.payload.get("text").and_then(Value::as_str).map(str::to_string) else {
                tracing::warn!("npc-action: 'command' message missing text payload");
                return;
            };
            tokio::spawn(async move {
                dispatcher::execute_command(state, &text).await;
            });
        }
        other => {
            tracing::debug!(msg_type = other, "npc-action: ignoring unrecognized bus message type");
        }
    }
}

/// Accepts either a bare JSON string payload or `{"content": "..."}`.
fn extract_text(payload: &Value) -> Option<String> {
    if let Some(s) = payload.as_str() {
        return Some(s.to_string());
    }
    payload.get("content").and_then(Value::as_str).map(str::to_string)
}
