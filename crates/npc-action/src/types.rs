//! Wire types for the NL-action-chat flow (ports Go `internal/agent/action.go`).

use serde::{Deserialize, Serialize};

/// One dispatcher command invocation, as emitted by the LLM's `actions`
/// array or synthesized from a parsed CLI-style command. Ports Go `Action`
/// (minus `CmdFunc`, which this port doesn't need — the dispatcher looks up
/// the command function by name in both cases).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ActionItem {
    pub name: String,
    #[serde(default)]
    pub args: Vec<String>,
}

/// Ports Go `ActionResponse`: the strict-JSON shape the NL system prompt
/// asks the LLM to reply with.
#[derive(Debug, Clone, Deserialize, Default)]
pub struct ActionResponse {
    #[serde(default)]
    pub message: String,
    #[serde(default)]
    pub actions: Vec<ActionItem>,
}
