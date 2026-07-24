//! Shared state for the action module: the OSC client, navigator/autopilot,
//! LLM client + chat history, and the single "active background task" slot
//! that `ExecuteActions`/`stopActiveTask` used to guard in Go.

use std::sync::atomic::AtomicBool;
use std::sync::{Arc, Mutex};

use npc_core::{Bus, Config};
use npc_llm::{ChatMessage, LlmClient};
use tokio::sync::Mutex as AsyncMutex;
use tokio_util::sync::CancellationToken;

use crate::autopilot::Autopilot;
use crate::controller::Controller;
use crate::navigator::Navigator;
use crate::osc::VrcClient;

/// A running cancellable action sequence (ports the Go CLI's
/// `activeStop`/`activeTaskDone` channel pair).
pub struct ActiveTask {
    pub token: CancellationToken,
    pub handle: tokio::task::JoinHandle<()>,
}

pub struct ActionState {
    pub bus: Bus,
    pub config: Arc<Config>,
    /// Parent of every action-sequence's cancellation token, so process
    /// shutdown also cancels any in-flight movement.
    pub shutdown: CancellationToken,

    pub vrc: Arc<VrcClient>,
    pub controller: Controller,
    pub navigator: Arc<Navigator>,
    pub autopilot: Autopilot,

    pub llm: LlmClient,
    pub static_system_prompt: String,
    pub chat_history: AsyncMutex<Vec<ChatMessage>>,
    /// Guards `ChatWithAction`: only one NL round-trip runs at a time: new
    /// requests are dropped (and logged) while one is in flight.
    pub chat_busy: AtomicBool,

    pub active: Mutex<Option<ActiveTask>>,
}

impl ActionState {
    /// Publishes an `npc:ui` / `action_log` line, mirroring every
    /// `fmt.Println`/`fmt.Printf` the Go CLI used to write to stdout.
    pub fn log(&self, text: impl Into<String>) {
        self.bus.publish("npc:ui", "action_log", serde_json::json!({ "text": text.into() }));
    }
}
