//! A small tool-calling registry, ported from Go `internal/tool`. Currently
//! registered with zero built-in tools (the Go original's gathering-schedule
//! tool is intentionally not ported yet), but the full call/execute plumbing
//! is wired up in [`crate::engine::ChatEngine`] so tools can be added later
//! without touching the turn loop.

use std::collections::HashMap;

use async_trait::async_trait;
use npc_llm::Tool as ToolDef;

/// A single callable tool: an OpenAI-style function definition plus an
/// executor that turns JSON-encoded arguments into a result string (or an
/// error, which the engine renders as `"Error: {err}"` back to the model —
/// matching the Go original's `chatWithTools` error handling).
#[async_trait]
pub trait ChatTool: Send + Sync {
    fn definition(&self) -> ToolDef;
    async fn execute(&self, args: &str) -> anyhow::Result<String>;
}

/// Name -> tool lookup, mirroring Go's `tool.Registry`.
#[derive(Default)]
pub struct ToolRegistry {
    tools: HashMap<String, Box<dyn ChatTool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    #[allow(dead_code)]
    pub fn register(&mut self, name: impl Into<String>, tool: Box<dyn ChatTool>) {
        self.tools.insert(name.into(), tool);
    }

    pub fn definitions(&self) -> Vec<ToolDef> {
        self.tools.values().map(|t| t.definition()).collect()
    }

    /// Execute `name` with `args`, always returning a string suitable to
    /// hand back to the model as a tool result (errors — including "tool not
    /// found" — are rendered inline rather than propagated, matching Go's
    /// `chatAgent.Chat` which does `result = fmt.Sprintf("Error: %v", err)`).
    pub async fn execute(&self, name: &str, args: &str) -> String {
        match self.tools.get(name) {
            Some(tool) => match tool.execute(args).await {
                Ok(result) => result,
                Err(err) => format!("Error: {err}"),
            },
            None => format!("Error: tool {name} not found"),
        }
    }
}
