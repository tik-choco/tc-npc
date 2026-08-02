//! Chat/dialogue module for tc-npc — a port of Go `agent-talk`
//! (`internal/agent/agent.go`'s `ChatAgent`). Subscribes to the bus for
//! memory updates (`agent:mem`), speech input (`agent:sense`), and
//! suspend/resume control (`agent:interrupt`); runs chat turns against an
//! OpenAI-compatible LLM via `npc-llm`; publishes responses back on
//! `agent:chat`.

pub mod affect;
mod engine;
mod module;
mod style;
mod tools;

use npc_core::{Module, ModuleCtx};

pub use affect::{AffectState, DriveKey};
pub use engine::ChatEngine;
pub use module::TalkModule;
pub use style::{is_silence, sanitize_reply, SILENCE_SENTINEL, SPOKEN_REPLY_RULES};
pub use tools::{ChatTool, ToolRegistry};

pub fn module(_ctx: &ModuleCtx) -> anyhow::Result<Box<dyn Module>> {
    Ok(Box::new(TalkModule))
}
