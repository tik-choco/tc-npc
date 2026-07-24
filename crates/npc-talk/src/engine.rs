//! The chat/dialogue engine itself — a faithful port of the original Go
//! `agent-talk` service's `ChatAgent`/`Chat()` (predecessor of this
//! unified binary).

use std::collections::HashMap;
use std::sync::OnceLock;

use npc_core::config::PromptEntry;
use npc_core::{msg, topic, Bus};
use npc_llm::{ChatContent, ChatMessage, LlmClient, ResponseMessage};
use regex::Regex;
use tokio::sync::Mutex;

use crate::tools::ToolRegistry;

/// State mutated over the lifetime of a conversation: message history plus
/// the latest short/long-term memory snapshots pushed in from `npc-memory`.
/// Guarded by a single mutex so an entire chat turn (including memory reads)
/// runs atomically — this both serializes concurrent turns and matches Go's
/// `ChatAgent.mu` covering `Chat`, `SetMemory`, and `SetLongTermMemory`
/// alike.
struct ChatState {
    history: Vec<ChatMessage>,
    short_term_memory: String,
    long_term_memory: String,
}

pub struct ChatEngine {
    llm: LlmClient,
    model: String,
    reasoning_effort: Option<String>,
    prompts: Vec<PromptEntry>,
    filter_prompt: Option<String>,
    history_size: usize,
    tools: ToolRegistry,
    bus: Bus,
    /// Rendered persona prompt for the active character, if any.
    persona: Option<String>,
    state: Mutex<ChatState>,
}

impl ChatEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        llm: LlmClient,
        model: String,
        reasoning_effort: Option<String>,
        prompts: Vec<PromptEntry>,
        filter_prompt_name: String,
        history_size: usize,
        tools: ToolRegistry,
        persona: Option<String>,
        bus: Bus,
    ) -> Self {
        let filter_prompt = prompts
            .iter()
            .find(|p| p.name == filter_prompt_name && !filter_prompt_name.is_empty())
            .map(|p| p.content.clone());

        Self {
            llm,
            model,
            reasoning_effort,
            prompts,
            filter_prompt,
            history_size,
            tools,
            bus,
            persona,
            state: Mutex::new(ChatState {
                history: Vec::new(),
                short_term_memory: String::new(),
                long_term_memory: String::new(),
            }),
        }
    }

    pub async fn set_short_term_memory(&self, content: String) {
        let mut guard = self.state.lock().await;
        guard.short_term_memory = content;
    }

    pub async fn set_long_term_memory(&self, content: String) {
        let mut guard = self.state.lock().await;
        guard.long_term_memory = content;
    }

    /// Run one full conversation turn: system prompts -> history -> user
    /// input -> tool-call loop -> optional filter pass -> publish +
    /// history update. Ported from Go `ChatAgent.Chat`.
    pub async fn chat(&self, input: &str) -> anyhow::Result<String> {
        let mut guard = match self.state.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::debug!("npc-talk: chat turn queued behind an in-progress turn");
                self.state.lock().await
            }
        };

        let vars = self.build_vars(&guard.short_term_memory, &guard.long_term_memory);

        let mut messages: Vec<ChatMessage> = Vec::new();

        let persona_text = self.persona.clone().unwrap_or_default();
        let persona_referenced = self.prompts.iter().any(|p| p.content.contains("{{persona}}"));
        if !persona_referenced && !persona_text.is_empty() {
            messages.push(ChatMessage::system(persona_text));
        }

        for p in &self.prompts {
            let content = fill_template(&p.content, &vars);
            if content.is_empty() {
                continue;
            }
            messages.push(ChatMessage::system(content));
        }

        messages.extend(guard.history.iter().cloned());

        let user_msg = ChatMessage::user(input.to_string());
        messages.push(user_msg.clone());

        let tool_defs = {
            let defs = self.tools.definitions();
            if defs.is_empty() {
                None
            } else {
                Some(defs)
            }
        };

        let mut resp = self.chat_with_tools(messages.clone(), tool_defs.clone()).await?;

        guard.history.push(user_msg);

        while resp.tool_calls.as_ref().is_some_and(|tc| !tc.is_empty()) {
            let tool_calls = resp.tool_calls.clone().unwrap_or_default();
            let assistant_msg = ChatMessage {
                role: "assistant".to_string(),
                content: ChatContent::Text(resp.content.clone().unwrap_or_default()),
                tool_calls: Some(tool_calls.clone()),
                tool_call_id: None,
            };
            messages.push(assistant_msg.clone());
            guard.history.push(assistant_msg);

            for tc in &tool_calls {
                tracing::info!(name = %tc.function.name, args = %tc.function.arguments, "npc-talk: tool call");
                let result = self.tools.execute(&tc.function.name, &tc.function.arguments).await;
                let tool_msg = ChatMessage::tool_result(tc.id.clone(), result);
                messages.push(tool_msg.clone());
                guard.history.push(tool_msg);
            }

            resp = self.chat_with_tools(messages.clone(), tool_defs.clone()).await?;
        }

        let mut final_content = resp.content.clone().unwrap_or_default();

        if let Some(filter_prompt) = &self.filter_prompt {
            if !final_content.is_empty() {
                let filter_messages = vec![
                    ChatMessage::system(filter_prompt.clone()),
                    ChatMessage::user(final_content.clone()),
                ];
                match self.chat_with_tools(filter_messages, None).await {
                    Ok(filtered) => {
                        final_content = filtered.content.unwrap_or_default();
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "npc-talk: filter pass failed, keeping unfiltered response");
                    }
                }
            }
        }

        if final_content.is_empty() {
            final_content = " ".to_string();
        }

        guard.history.push(ChatMessage::assistant(final_content.clone()));
        trim_history(&mut guard.history, self.history_size);

        drop(guard);

        self.bus.publish(
            topic::CHAT,
            msg::CHAT_RESPONSE,
            serde_json::json!({ "content": final_content, "input": input }),
        );
        self.bus.publish(
            topic::CHAT,
            msg::CHAT_LOG,
            serde_json::json!({
                "input": input,
                "output": final_content,
                "time": chrono::Utc::now().to_rfc3339(),
            }),
        );

        Ok(final_content)
    }

    fn build_vars(&self, short_term_memory: &str, long_term_memory: &str) -> HashMap<String, String> {
        let mut vars = HashMap::new();
        vars.insert(
            "session_meta".to_string(),
            chrono::Local::now().format("%Y-%m-%d %H:%M:%S (%a)").to_string(),
        );
        if !short_term_memory.is_empty() {
            vars.insert("short_term_memory".to_string(), short_term_memory.to_string());
        }
        if !long_term_memory.is_empty() {
            vars.insert("long_term_memory".to_string(), long_term_memory.to_string());
        }
        vars.insert("persona".to_string(), self.persona.clone().unwrap_or_default());
        vars
    }

    async fn chat_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<npc_llm::Tool>>,
    ) -> anyhow::Result<ResponseMessage> {
        let mut req = npc_llm::ChatRequest::new(self.model.clone(), messages);
        req.tools = tools;
        req.reasoning_effort = self.reasoning_effort.clone();

        let resp = self.llm.chat(req).await?;
        let choice = resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no choices returned"))?;
        Ok(choice.message)
    }
}

/// `{{key}}` placeholder substitution, matching Go `fillTemplate`: unknown
/// keys resolve to the empty string, and the result is trimmed.
pub fn fill_template(tpl: &str, vars: &HashMap<String, String>) -> String {
    static RE: OnceLock<Regex> = OnceLock::new();
    let re = RE.get_or_init(|| Regex::new(r"\{\{([a-zA-Z0-9_-]+)\}\}").unwrap());
    let replaced = re.replace_all(tpl, |caps: &regex::Captures| {
        vars.get(&caps[1]).cloned().unwrap_or_default()
    });
    replaced.trim().to_string()
}

/// Two-pass history trim, ported exactly from Go `ChatAgent.Chat`'s tail:
/// first prefer cutting at a user-message boundary within the last
/// `history_size` messages, then hard-cut to the last `history_size`
/// messages if that still leaves more than `history_size * 2`.
pub fn trim_history(history: &mut Vec<ChatMessage>, history_size: usize) {
    if history.len() > history_size {
        let start = history.len() - history_size;
        for i in start..history.len() {
            if history[i].role == "user" {
                history.drain(0..i);
                break;
            }
        }
        if history.len() > history_size * 2 {
            let cut = history.len() - history_size;
            history.drain(0..cut);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn vars(pairs: &[(&str, &str)]) -> HashMap<String, String> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn fills_known_placeholders() {
        let tpl = "Now: {{session_meta}}. Memory: {{short_term_memory}}.";
        let v = vars(&[("session_meta", "2026-07-24 12:00:00 (Fri)"), ("short_term_memory", "hello")]);
        assert_eq!(
            fill_template(tpl, &v),
            "Now: 2026-07-24 12:00:00 (Fri). Memory: hello."
        );
    }

    #[test]
    fn unknown_placeholder_becomes_empty() {
        let tpl = "Persona: {{persona}} end";
        let v = vars(&[]);
        assert_eq!(fill_template(tpl, &v), "Persona:  end");
    }

    #[test]
    fn trims_result() {
        let tpl = "  {{missing}}  ";
        let v = vars(&[]);
        assert_eq!(fill_template(tpl, &v), "");
    }

    #[test]
    fn ignores_non_placeholder_braces() {
        let tpl = "literal {not a placeholder} {{persona}}";
        let v = vars(&[("persona", "P")]);
        assert_eq!(fill_template(tpl, &v), "literal {not a placeholder} P");
    }

    fn user(text: &str) -> ChatMessage {
        ChatMessage::user(text.to_string())
    }
    fn assistant(text: &str) -> ChatMessage {
        ChatMessage::assistant(text.to_string())
    }

    #[test]
    fn trim_history_noop_when_within_limit() {
        let mut h = vec![user("a"), assistant("b")];
        trim_history(&mut h, 20);
        assert_eq!(h.len(), 2);
    }

    #[test]
    fn trim_history_cuts_at_user_boundary() {
        // 5 pairs (10 messages), history_size = 4: start = 10-4 = 6.
        // history[6] is the 4th user message (index 6 = pair 4's user turn).
        let mut h = Vec::new();
        for i in 0..5 {
            h.push(user(&format!("u{i}")));
            h.push(assistant(&format!("a{i}")));
        }
        trim_history(&mut h, 4);
        // start=6 -> history[6] = u3 (role user) -> drain(0..6) -> remaining = [u3,a3,u4,a4]
        assert_eq!(h.len(), 4);
        assert_eq!(h[0].content.as_text(), "u3");
        assert_eq!(h[0].role, "user");
    }

    #[test]
    fn trim_history_hard_cuts_when_no_user_boundary_found() {
        // No user-role messages at all within [start, len) range: force the
        // second pass by using an all-assistant tail.
        let mut h = vec![user("u0")];
        for i in 0..10 {
            h.push(assistant(&format!("a{i}")));
        }
        // len = 11, history_size = 3: start = 8, history[8..11] are all
        // assistant -> first pass finds nothing -> history unchanged (11) ->
        // 11 > 3*2=6 -> hard cut to last 3.
        trim_history(&mut h, 3);
        assert_eq!(h.len(), 3);
        assert_eq!(h[0].content.as_text(), "a7");
        assert_eq!(h[2].content.as_text(), "a9");
    }

    #[test]
    fn trim_history_zero_size_empties_history() {
        let mut h = vec![user("u0"), assistant("a0")];
        trim_history(&mut h, 0);
        assert!(h.is_empty());
    }
}
