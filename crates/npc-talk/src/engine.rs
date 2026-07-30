//! The chat/dialogue engine itself — a faithful port of the original Go
//! `agent-talk` service's `ChatAgent`/`Chat()` (predecessor of this
//! unified binary).

use std::collections::HashMap;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

use npc_core::config::PromptEntry;
use npc_core::{msg, topic, Bus};
use npc_llm::{ChatContent, ChatMessage, LlmClient, ResponseMessage};
use regex::Regex;
use tokio::sync::Mutex;

use crate::affect::{AffectState, PartnerAffect};
use crate::style::{is_silence, sanitize_reply, SPOKEN_REPLY_RULES};
use crate::tools::ToolRegistry;

/// State mutated over the lifetime of a conversation: message history plus
/// the latest short/long-term memory snapshots pushed in from `npc-memory`,
/// plus the emotion-drive state (`npc_core::config::AffectConfig`; see
/// `crate::affect`). Guarded by a single mutex so an entire chat turn
/// (including memory reads) runs atomically — this both serializes
/// concurrent turns and matches Go's `ChatAgent.mu` covering `Chat`,
/// `SetMemory`, and `SetLongTermMemory` alike.
struct ChatState {
    history: Vec<ChatMessage>,
    short_term_memory: String,
    long_term_memory: String,
    /// Recalled profile for the person currently in view, pushed in by
    /// npc-memory on `topic::MEM`/`msg::PERSON_MEMORY` (person-memory
    /// contract §6). Same "empty string when unset" treatment as
    /// `short_term_memory`/`long_term_memory`.
    person_memory: String,
    /// Rendered persona prompt for the active character, or `None` when no
    /// character is active. Lives here rather than as a plain `ChatEngine`
    /// field so a live character switch (`crate::module`'s `CONFIG_UPDATED`
    /// handler, via [`ChatEngine::set_persona`]) goes through the same lock
    /// an in-progress turn already holds for its whole duration, instead of
    /// racing it: a turn in flight finishes with whichever persona it
    /// started with, and the very next turn picks up the new one. Without
    /// this, switching mid-turn would let one turn observe a persona for
    /// part of message-building and a different one for the rest.
    persona: Option<String>,
    /// Per-partner drive state. Conversation `history` above stays shared
    /// across partners on purpose — an NPC in a room hears everyone, so the
    /// transcript is one thread — but the internal state follows whoever is
    /// actually being spoken to (see [`PartnerAffect`]).
    affect: PartnerAffect,
}

pub struct ChatEngine {
    llm: LlmClient,
    model: String,
    prompts: Vec<PromptEntry>,
    filter_prompt: Option<String>,
    history_size: usize,
    tools: ToolRegistry,
    bus: Bus,
    /// `config.language` reply instruction, pushed as its own system message
    /// after the configured prompts so it isn't buried mid-persona. `None`
    /// when the language is `auto`.
    language_instruction: Option<&'static str>,
    /// `config.talk.style_rules` — gates the built-in
    /// [`SPOKEN_REPLY_RULES`] system message. On by default because
    /// `talk.prompts` is empty out of the box: without it the model is left
    /// with the persona alone and writes prose (stage directions, markdown,
    /// written register) into what is about to be read aloud.
    style_rules: bool,
    /// `config.talk.allow_silence` — lets a turn end without the NPC saying
    /// anything (see [`crate::style::is_silence`]). Off restores the old
    /// always-answer behavior, where a wordless reply was published as a
    /// single space.
    allow_silence: bool,
    /// `config.talk.affect.enabled` — gates both the `AffectState::update`
    /// call and the `to_prompt()` system message. When `false`, behavior is
    /// bit-for-bit identical to a build without the affect model.
    affect_enabled: bool,
    /// `config.talk.affect.forced_closure` — gates the "answer without
    /// calling the LLM" short-circuit. Only consulted when `affect_enabled`
    /// is also `true`.
    affect_forced_closure: bool,
    /// `config.talk.affect.absence_timeout_secs` — silence past this counts
    /// as the partner having left. Zero disables the check.
    absence_timeout: Duration,
    state: Mutex<ChatState>,
}

impl ChatEngine {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        llm: LlmClient,
        model: String,
        prompts: Vec<PromptEntry>,
        filter_prompt_name: String,
        history_size: usize,
        tools: ToolRegistry,
        persona: Option<String>,
        language: &str,
        bus: Bus,
        affect_config: npc_core::config::AffectConfig,
        style_rules: bool,
        allow_silence: bool,
    ) -> Self {
        let filter_prompt = prompts
            .iter()
            .find(|p| p.name == filter_prompt_name && !filter_prompt_name.is_empty())
            .map(|p| p.content.clone());

        if affect_config.enabled {
            // Published up front so the web UI's affect panel isn't empty
            // while it waits for the first conversation turn.
            bus.publish(topic::UI, msg::AFFECT_STATE, AffectState::default().snapshot());
        }

        Self {
            llm,
            model,
            prompts,
            filter_prompt,
            history_size,
            tools,
            bus,
            language_instruction: npc_core::config::language_instruction(language),
            style_rules,
            allow_silence,
            affect_enabled: affect_config.enabled,
            affect_forced_closure: affect_config.forced_closure,
            absence_timeout: Duration::from_secs(affect_config.absence_timeout_secs),
            state: Mutex::new(ChatState {
                history: Vec::new(),
                short_term_memory: String::new(),
                long_term_memory: String::new(),
                person_memory: String::new(),
                persona,
                affect: PartnerAffect::default(),
            }),
        }
    }

    /// Publish the current [`AffectState`] snapshot on `npc:ui` /
    /// `affect_state` for the web UI's real-time affect panel. No-op when
    /// `config.talk.affect.enabled` is `false`.
    fn publish_affect_snapshot(&self, affect: &PartnerAffect) {
        if !self.affect_enabled {
            return;
        }
        self.bus.publish(topic::UI, msg::AFFECT_STATE, affect.snapshot());
    }

    /// Poll for the conversation partner having walked away, publishing an
    /// updated affect frame if they have. Driven by a timer in
    /// [`crate::module`] rather than by a turn, because the whole point is to
    /// notice a turn that never came.
    ///
    /// Uses `try_lock` and gives up if a turn currently holds the state: the
    /// partner is demonstrably not absent while their utterance is being
    /// answered, and the next tick will pick it up anyway.
    pub async fn check_partner_absence(&self) {
        if !self.affect_enabled || self.absence_timeout.is_zero() {
            return;
        }
        let Ok(mut guard) = self.state.try_lock() else {
            return;
        };
        if let Some(absence) = guard.affect.check_absence(Instant::now(), self.absence_timeout) {
            tracing::info!(
                who = ?absence.who,
                idle_secs = absence.idle.as_secs(),
                "npc-talk: conversation partner went quiet, treating them as away",
            );
            self.publish_affect_snapshot(&guard.affect);
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

    pub async fn set_person_memory(&self, content: String) {
        let mut guard = self.state.lock().await;
        guard.person_memory = content;
    }

    /// Install a new persona prompt, or clear it entirely with `None`.
    /// Called from `crate::module`'s `CONFIG_UPDATED` handler when
    /// `config.character.active_id` changes, so a switch made in the web
    /// UI's キャラ tab takes effect on the next turn instead of requiring the
    /// binary to be restarted.
    ///
    /// Takes the same lock a chat turn holds for its whole duration (see
    /// [`ChatState::persona`]), so a switch arriving mid-turn simply waits
    /// for that turn to finish rather than swapping the persona out from
    /// under it.
    pub async fn set_persona(&self, persona: Option<String>) {
        let mut guard = self.state.lock().await;
        guard.persona = persona;
    }

    /// Current persona prompt, if any. Only compiled for tests: production
    /// code has no need to read this back (it is consumed inside
    /// `chat_with_speaker`), but `crate::module`'s tests need some way to
    /// confirm a `CONFIG_UPDATED` character switch actually reached the
    /// engine, since there is no other externally observable sign of it
    /// until the next LLM request goes out.
    #[cfg(test)]
    pub async fn persona(&self) -> Option<String> {
        self.state.lock().await.persona.clone()
    }

    /// Run one full conversation turn: system prompts -> history -> user
    /// input -> tool-call loop -> optional filter pass -> publish +
    /// history update. Ported from Go `ChatAgent.Chat`.
    pub async fn chat(&self, input: &str) -> anyhow::Result<String> {
        self.chat_with_speaker(input, None).await
    }

    /// Same as [`Self::chat`], but with an optional `speaker` (who said
    /// `input`, per the person-memory contract's `agent:sense`/`speech`
    /// `speaker` field) threaded through to the published `chat_log` so
    /// npc-memory can attribute the turn to a person. `chat` itself stays
    /// speaker-less so every existing call site keeps behaving identically.
    ///
    /// Delegates to [`Self::chat_with_context`] with no `request_id` — the
    /// two remaining production call sites (`chat`'s own callers, none of
    /// which currently exist outside tests, and any future self-initiated
    /// turn) have no WS client waiting on them.
    pub async fn chat_with_speaker(&self, input: &str, speaker: Option<&str>) -> anyhow::Result<String> {
        self.chat_with_context(input, speaker, None).await
    }

    /// Same as [`Self::chat_with_speaker`], plus an optional `request_id`:
    /// the WS API's per-request correlation id (npc-server's `ws.rs` mints
    /// one per `input` frame and stamps it onto the `agent:sense`/`speech`
    /// payload as `request_id`; see `crate::module::extract_request_id`).
    ///
    /// Carried purely as a call parameter, *not* through [`ChatState`] the
    /// way `speaker` conceptually informs `affect` or `persona` informs the
    /// system prompt: unlike those, `request_id` has no bearing on what gets
    /// said, only on which reply the id belongs to, and it only needs to
    /// live for the duration of this one call. Stashing it in shared state
    /// instead would risk a second queued turn's id clobbering this one's
    /// before this turn gets around to publishing — the whole failure mode
    /// this plumbing exists to avoid. Passing it down the call stack alongside
    /// `input`/`speaker` sidesteps that entirely: it travels with the
    /// utterance it belongs to, all the way to whichever of `chat_response`
    /// or `chat_silent` ends this turn.
    ///
    /// A turn the NPC started on its own (a scheduler announcement, a vision
    /// remark) has no requester and passes `None` here; that `None` is
    /// carried through to the published payload as a missing `request_id`
    /// field rather than an empty string, matching how npc-server's
    /// `bus_forward` distinguishes "nobody is waiting" from "waiting on the
    /// empty-string request".
    pub async fn chat_with_context(
        &self,
        input: &str,
        speaker: Option<&str>,
        request_id: Option<&str>,
    ) -> anyhow::Result<String> {
        let result = self.run_turn(input, speaker, request_id).await;

        // A turn that fails outright publishes neither `chat_response` nor
        // `chat_silent`, so a client waiting on this request would otherwise
        // never hear anything back — its `response` frame simply never
        // arrives. Announce the failure so npc-server can answer it with
        // `status: "error"` instead. Only when there *is* a requester: a
        // failed self-initiated turn (a scheduler announcement, a vision
        // remark) has nobody waiting, and the error is already logged.
        //
        // Wrapping the whole turn rather than reporting at each `?` site is
        // deliberate — every failure inside `run_turn`, present and future,
        // is covered by construction instead of by remembering to add a
        // publish beside each new error path.
        if let (Err(err), Some(request_id)) = (&result, request_id) {
            self.bus.publish(
                topic::CHAT,
                msg::CHAT_ERROR,
                serde_json::json!({ "request_id": request_id, "message": err.to_string() }),
            );
        }

        result
    }

    /// The turn itself. Split out of [`Self::chat_with_context`] so that
    /// wrapper can observe any error on the way out; call that, not this.
    async fn run_turn(
        &self,
        input: &str,
        speaker: Option<&str>,
        request_id: Option<&str>,
    ) -> anyhow::Result<String> {
        let mut guard = match self.state.try_lock() {
            Ok(guard) => guard,
            Err(_) => {
                tracing::debug!("npc-talk: chat turn queued behind an in-progress turn");
                self.state.lock().await
            }
        };

        if self.affect_enabled {
            let now = Instant::now();
            // Catch up on an absence the timer may not have run for (or that
            // it skipped because a turn held the lock) before scoring this
            // utterance, so a partner returning after a long gap resumes from
            // a settled state rather than from mid-conversation.
            guard.affect.check_absence(now, self.absence_timeout);
            // Detect the partner changing *before* folding this utterance in,
            // so the turn is scored against the new partner's state rather
            // than the outgoing one's.
            if let Some(switch) = guard.affect.observe(speaker) {
                tracing::info!(
                    from = ?switch.from,
                    to = %switch.to,
                    returning = switch.returning,
                    "npc-talk: conversation partner changed",
                );
            }
            guard.affect.update(input, now);
            self.publish_affect_snapshot(&guard.affect);

            if self.affect_forced_closure {
                if let Some(reply) = guard.affect.state().forced_closure_reply(input) {
                    // The state machine's own "……" (the conversation is over
                    // and the partner is just acking) is a wordless line: with
                    // silence enabled it ends the turn outright instead of
                    // handing TTS something with nothing in it to say.
                    if self.allow_silence && is_silence(&reply) {
                        guard.history.push(ChatMessage::user(input.to_string()));
                        trim_history(&mut guard.history, self.history_size);
                        drop(guard);
                        self.publish_silence(input, "closing", speaker, request_id);
                        return Ok(String::new());
                    }

                    guard.history.push(ChatMessage::user(input.to_string()));
                    guard.history.push(ChatMessage::assistant(reply.clone()));
                    trim_history(&mut guard.history, self.history_size);
                    drop(guard);

                    self.bus.publish(
                        topic::CHAT,
                        msg::CHAT_RESPONSE,
                        chat_response_payload(&reply, input, request_id),
                    );
                    self.bus.publish(topic::CHAT, msg::CHAT_LOG, chat_log_payload(input, &reply, speaker));

                    return Ok(reply);
                }
            }
        }

        let mut vars = self.build_vars(&guard.short_term_memory, &guard.long_term_memory, &guard.person_memory);

        let mut messages: Vec<ChatMessage> = Vec::new();

        // Read under the same lock `set_persona` writes through, so this
        // turn sees either the persona it started with or a switch that
        // fully landed before the turn began — never a half-applied one.
        let persona_text = guard.persona.clone().unwrap_or_default();
        vars.insert("persona".to_string(), persona_text.clone());
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

        if let Some(rules) = self.style_message() {
            messages.push(rules);
        }

        if self.affect_enabled {
            messages.push(ChatMessage::system(guard.affect.state().to_prompt()));
        }

        if let Some(instruction) = self.language_instruction {
            messages.push(ChatMessage::system(instruction));
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
                // The filter pass rewrites the reply, so it needs the same
                // language instruction — otherwise it can hand back a
                // rewrite in whatever language the filter prompt is in.
                let mut filter_system = filter_prompt.clone();
                if let Some(instruction) = self.language_instruction {
                    filter_system.push_str("\n\n");
                    filter_system.push_str(instruction);
                }
                let filter_messages = vec![
                    ChatMessage::system(filter_system),
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

        // Last line of defence for the spoken-output rules above: the reply
        // is about to be read aloud by TTS and shown as a chat bubble, and a
        // small local model leaks stage directions ("（微笑む）"), markdown,
        // and leading blank lines however the prompt is worded. Applied after
        // the filter pass so a rewrite can't reintroduce them, and before the
        // history push so the model doesn't learn from its own leftovers.
        final_content = sanitize_reply(&final_content);

        // The NPC decided not to speak — either explicitly (the `<silence>`
        // tag from the reply rules: the conversation is over, the utterance
        // wasn't addressed to it, the mic caught a fragment) or by producing
        // nothing sayable. Publish no reply at all: nothing spoken, no
        // bubble, and no assistant turn in the history, since it didn't take
        // one. The user's utterance is still recorded, so what was said is
        // still remembered even though it went unanswered.
        if self.allow_silence && is_silence(&final_content) {
            trim_history(&mut guard.history, self.history_size);
            drop(guard);
            self.publish_silence(input, "declined", speaker, request_id);
            return Ok(String::new());
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
            chat_response_payload(&final_content, input, request_id),
        );
        self.bus.publish(
            topic::CHAT,
            msg::CHAT_LOG,
            chat_log_payload(input, &final_content, speaker),
        );

        Ok(final_content)
    }

    /// Announce a turn the NPC chose not to answer: `chat_silent` for the
    /// web UI (so its typing indicator stops and the operator can see the
    /// turn was deliberately left alone), plus the usual `chat_log` with an
    /// empty output so npc-memory still records what was said to it.
    ///
    /// `reason` is `"closing"` (the farewell state machine) or `"declined"`
    /// (the reply itself asked for silence).
    ///
    /// Carries `request_id` the same as the spoken-reply path: a turn the
    /// NPC deliberately left unanswered still *ends* the request a WS client
    /// may be blocked on (npc-server's `bus_forward` turns `chat_silent` into
    /// the WS `response` frame same as `chat_response` does — see its
    /// handling of `msg::CHAT_SILENT`). Forgetting to thread it through here
    /// specifically would leave that client's `input` waiting forever
    /// whenever the NPC's answer to it was silence.
    fn publish_silence(&self, input: &str, reason: &str, speaker: Option<&str>, request_id: Option<&str>) {
        tracing::info!(reason, input = %input, request_id = ?request_id, "npc-talk: staying silent this turn");
        self.bus.publish(
            topic::CHAT,
            msg::CHAT_SILENT,
            chat_silent_payload(input, reason, request_id),
        );
        self.bus.publish(topic::CHAT, msg::CHAT_LOG, chat_log_payload(input, "", speaker));
    }

    /// The built-in spoken-reply rules as a system message, or `None` when
    /// `config.talk.style_rules` is off. Pushed after the configured
    /// `talk.prompts` (so a hand-written prompt is read first) and before the
    /// affect state, which speaks in the same voice about the same turn.
    fn style_message(&self) -> Option<ChatMessage> {
        self.style_rules.then(|| ChatMessage::system(SPOKEN_REPLY_RULES.to_string()))
    }

    fn build_vars(
        &self,
        short_term_memory: &str,
        long_term_memory: &str,
        person_memory: &str,
    ) -> HashMap<String, String> {
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
        if !person_memory.is_empty() {
            vars.insert("person_memory".to_string(), person_memory.to_string());
        }
        // `persona` is deliberately not filled in here: unlike the plain
        // `&str` snapshots above, it can change concurrently via
        // `set_persona`, so the caller inserts it straight from the same
        // `ChatState` guard it already holds rather than this function
        // taking a fourth snapshot parameter that could drift from it.
        vars
    }

    async fn chat_with_tools(
        &self,
        messages: Vec<ChatMessage>,
        tools: Option<Vec<npc_llm::Tool>>,
    ) -> anyhow::Result<ResponseMessage> {
        let mut req = npc_llm::ChatRequest::new(self.model.clone(), messages);
        req.tools = tools;

        let resp = self.llm.chat(req).await?;
        let choice = resp
            .choices
            .into_iter()
            .next()
            .ok_or_else(|| anyhow::anyhow!("no choices returned"))?;
        Ok(choice.message)
    }
}

/// Build the `topic::CHAT`/`msg::CHAT_LOG` payload, carrying `speaker` along
/// (person-memory contract §2) when the turn's input came with one attached.
/// `speaker` is omitted entirely rather than emitted as `null`/empty so
/// npc-memory's "no speaker" case matches the pre-existing payload shape.
fn chat_log_payload(input: &str, output: &str, speaker: Option<&str>) -> serde_json::Value {
    let mut payload = serde_json::json!({
        "input": input,
        "output": output,
        "time": chrono::Utc::now().to_rfc3339(),
    });
    if let Some(speaker) = speaker {
        payload["speaker"] = serde_json::Value::String(speaker.to_string());
    }
    payload
}

/// Build the `topic::CHAT`/`msg::CHAT_RESPONSE` payload, carrying the WS
/// API's per-request `request_id` along the same way [`chat_log_payload`]
/// carries `speaker`: present (as a sibling of the existing `content`/`input`
/// keys, matching the field npc-server's `ws.rs` stamps onto the incoming
/// `agent:sense`/`speech` payload) only when this turn was requested by a
/// client, omitted entirely otherwise. npc-server's `bus_forward` reads a
/// missing field as "no client is waiting on this reply" — a self-initiated
/// turn (scheduler announcement, vision remark) must produce exactly that,
/// not an empty-string id that would misread as a real (if blank) request.
fn chat_response_payload(content: &str, input: &str, request_id: Option<&str>) -> serde_json::Value {
    let mut payload = serde_json::json!({ "content": content, "input": input });
    if let Some(request_id) = request_id {
        payload["request_id"] = serde_json::Value::String(request_id.to_string());
    }
    payload
}

/// Same treatment as [`chat_response_payload`], for the `chat_silent` side
/// of a turn (see [`ChatEngine::publish_silence`]): a request the NPC
/// answered with silence still needs its `request_id` released back to
/// npc-server so a waiting WS client isn't left hanging forever.
fn chat_silent_payload(input: &str, reason: &str, request_id: Option<&str>) -> serde_json::Value {
    let mut payload = serde_json::json!({ "reason": reason, "input": input });
    if let Some(request_id) = request_id {
        payload["request_id"] = serde_json::Value::String(request_id.to_string());
    }
    payload
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

    #[test]
    fn chat_log_payload_includes_speaker_when_present() {
        let payload = chat_log_payload("hi", "hello", Some("太郎"));
        assert_eq!(payload["input"], "hi");
        assert_eq!(payload["output"], "hello");
        assert_eq!(payload["speaker"], "太郎");
    }

    #[test]
    fn chat_log_payload_omits_speaker_field_when_absent() {
        let payload = chat_log_payload("hi", "hello", None);
        assert!(payload.get("speaker").is_none());
    }

    #[test]
    fn chat_response_payload_includes_request_id_when_present() {
        let payload = chat_response_payload("hello", "hi", Some("req-1"));
        assert_eq!(payload["content"], "hello");
        assert_eq!(payload["input"], "hi");
        assert_eq!(payload["request_id"], "req-1");
    }

    #[test]
    fn chat_response_payload_omits_request_id_field_when_absent() {
        // No client is waiting on a self-initiated turn, so the field must
        // be missing entirely rather than an empty-string placeholder —
        // npc-server's bus_forward reads a missing field as "nobody to
        // reply to".
        let payload = chat_response_payload("hello", "hi", None);
        assert!(payload.get("request_id").is_none());
    }

    #[test]
    fn chat_silent_payload_includes_request_id_when_present() {
        let payload = chat_silent_payload("hi", "declined", Some("req-2"));
        assert_eq!(payload["reason"], "declined");
        assert_eq!(payload["input"], "hi");
        assert_eq!(payload["request_id"], "req-2");
    }

    #[test]
    fn chat_silent_payload_omits_request_id_field_when_absent() {
        let payload = chat_silent_payload("hi", "declined", None);
        assert!(payload.get("request_id").is_none());
    }

    fn test_engine() -> ChatEngine {
        ChatEngine::new(
            npc_llm::LlmClient::new("http://localhost:0/v1", ""),
            "test-model".to_string(),
            Vec::new(),
            String::new(),
            10,
            crate::tools::ToolRegistry::new(),
            None,
            "auto",
            npc_core::Bus::new(),
            npc_core::config::AffectConfig::default(),
            true,
            true,
        )
    }

    /// Drives the real turn path (the forced-closure branch returns before
    /// any LLM call, so this needs no network) to check the whole
    /// end-a-conversation story: the farewell is answered, and the "うん"
    /// after it is met with silence instead of a spoken "……".
    #[tokio::test]
    async fn a_conversation_that_has_ended_stops_producing_replies() {
        let bus = npc_core::Bus::new();
        let engine = ChatEngine::new(
            npc_llm::LlmClient::new("http://localhost:0/v1", ""),
            "test-model".to_string(),
            Vec::new(),
            String::new(),
            10,
            crate::tools::ToolRegistry::new(),
            None,
            "auto",
            bus.clone(),
            npc_core::config::AffectConfig::default(),
            true,
            true,
        );
        let mut rx = bus.subscribe();

        assert_eq!(engine.chat("じゃあね").await.unwrap(), "またね。");
        assert_eq!(engine.chat("うん").await.unwrap(), "");

        let types: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|m| m.env.r#type)
            .collect();
        // The farewell was spoken; the ack that followed produced a
        // `chat_silent` and no `chat_response` of its own.
        assert_eq!(
            types.iter().filter(|t| *t == msg::CHAT_RESPONSE).count(),
            1,
            "only the farewell should be spoken: {types:?}"
        );
        assert_eq!(types.iter().filter(|t| *t == msg::CHAT_SILENT).count(), 1, "{types:?}");
        // Both turns are still logged, so what was said is remembered even
        // though the second went unanswered.
        assert_eq!(types.iter().filter(|t| *t == msg::CHAT_LOG).count(), 2, "{types:?}");
    }

    /// The whole point of this plumbing: a WS client's `request_id` must
    /// reach it back whichever way the turn ends. Drives the same two-turn
    /// farewell sequence as the test above (forced-closure "answer" then
    /// forced-closure "silence", both reachable with no LLM call), but reads
    /// `request_id` back off each published payload instead of just counting
    /// message types — `chat_silent` is the outcome most likely to be
    /// forgotten, so it gets its own assertion here rather than being
    /// lumped in with the spoken reply.
    #[tokio::test]
    async fn request_id_survives_to_both_chat_response_and_chat_silent() {
        let bus = npc_core::Bus::new();
        let engine = ChatEngine::new(
            npc_llm::LlmClient::new("http://localhost:0/v1", ""),
            "test-model".to_string(),
            Vec::new(),
            String::new(),
            10,
            crate::tools::ToolRegistry::new(),
            None,
            "auto",
            bus.clone(),
            npc_core::config::AffectConfig::default(),
            true,
            true,
        );
        let mut rx = bus.subscribe();

        // Two distinct ids, one per turn — each must come back attached to
        // the reply *it* produced, not the other turn's.
        engine.chat_with_context("じゃあね", None, Some("req-1")).await.unwrap();
        engine.chat_with_context("うん", None, Some("req-2")).await.unwrap();

        let messages: Vec<npc_core::BusMessage> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

        let response = messages
            .iter()
            .find(|m| m.env.r#type == msg::CHAT_RESPONSE)
            .expect("the farewell should have produced a chat_response");
        assert_eq!(response.env.payload["request_id"], "req-1");

        let silent = messages
            .iter()
            .find(|m| m.env.r#type == msg::CHAT_SILENT)
            .expect("the ack should have produced a chat_silent");
        assert_eq!(
            silent.env.payload["request_id"], "req-2",
            "a turn answered with silence still has to release the client waiting on it"
        );
    }

    /// The other half of the contract: a turn nobody requested (this test
    /// stands in for a scheduler announcement or a vision-triggered remark)
    /// must publish *no* `request_id` field at all, on either outcome —
    /// not `null`, not `""`. npc-server's bus_forward treats a missing field
    /// as "nobody is waiting", which is the truth for this turn.
    #[tokio::test]
    async fn absent_request_id_publishes_no_request_id_field_on_either_outcome() {
        let bus = npc_core::Bus::new();
        let engine = ChatEngine::new(
            npc_llm::LlmClient::new("http://localhost:0/v1", ""),
            "test-model".to_string(),
            Vec::new(),
            String::new(),
            10,
            crate::tools::ToolRegistry::new(),
            None,
            "auto",
            bus.clone(),
            npc_core::config::AffectConfig::default(),
            true,
            true,
        );
        let mut rx = bus.subscribe();

        engine.chat_with_context("じゃあね", None, None).await.unwrap();
        engine.chat_with_context("うん", None, None).await.unwrap();

        let messages: Vec<npc_core::BusMessage> = std::iter::from_fn(|| rx.try_recv().ok()).collect();

        let response = messages
            .iter()
            .find(|m| m.env.r#type == msg::CHAT_RESPONSE)
            .expect("the farewell should have produced a chat_response");
        assert!(response.env.payload.get("request_id").is_none());

        let silent = messages
            .iter()
            .find(|m| m.env.r#type == msg::CHAT_SILENT)
            .expect("the ack should have produced a chat_silent");
        assert!(silent.env.payload.get("request_id").is_none());
    }

    #[tokio::test]
    async fn allow_silence_off_keeps_answering_every_turn() {
        let bus = npc_core::Bus::new();
        let engine = ChatEngine::new(
            npc_llm::LlmClient::new("http://localhost:0/v1", ""),
            "test-model".to_string(),
            Vec::new(),
            String::new(),
            10,
            crate::tools::ToolRegistry::new(),
            None,
            "auto",
            bus.clone(),
            npc_core::config::AffectConfig::default(),
            true,
            false,
        );
        let mut rx = bus.subscribe();

        engine.chat("じゃあね").await.unwrap();
        assert_eq!(engine.chat("うん").await.unwrap(), "……");

        let types: Vec<String> = std::iter::from_fn(|| rx.try_recv().ok())
            .map(|m| m.env.r#type)
            .collect();
        assert_eq!(types.iter().filter(|t| *t == msg::CHAT_RESPONSE).count(), 2, "{types:?}");
        assert!(!types.iter().any(|t| t == msg::CHAT_SILENT), "{types:?}");
    }

    #[test]
    fn style_rules_are_injected_by_default_and_can_be_turned_off() {
        let engine = test_engine();
        let message = engine.style_message().expect("rules injected by default");
        assert!(message.content.as_text().contains("ト書き"));
        assert_eq!(message.role, "system");

        let mut engine = test_engine();
        engine.style_rules = false;
        assert!(engine.style_message().is_none());
    }

    #[test]
    fn build_vars_includes_person_memory_when_set() {
        let engine = test_engine();
        let vars = engine.build_vars("", "", "太郎について: 犬好き");
        assert_eq!(vars.get("person_memory").map(String::as_str), Some("太郎について: 犬好き"));
    }

    #[test]
    fn build_vars_omits_person_memory_when_empty() {
        let engine = test_engine();
        let vars = engine.build_vars("", "", "");
        assert!(!vars.contains_key("person_memory"));
    }

    #[test]
    fn fill_template_expands_person_memory_placeholder() {
        let engine = test_engine();
        let vars = engine.build_vars("", "", "太郎: 犬好き");
        let tpl = "Person: {{person_memory}}";
        assert_eq!(fill_template(tpl, &vars), "Person: 太郎: 犬好き");
    }

    /// A turn that fails outright publishes neither `chat_response` nor
    /// `chat_silent`, so without `chat_error` the client that asked for it
    /// waits on a `response` frame that never comes. `test_engine`'s endpoint
    /// (`localhost:0`) is unreachable, so an ordinary utterance — unlike the
    /// forced-closure farewell above, which returns before any LLM call —
    /// fails inside the turn and exercises exactly that path.
    #[tokio::test]
    async fn a_failed_turn_reports_the_error_to_its_requester() {
        let bus = npc_core::Bus::new();
        let engine = ChatEngine::new(
            npc_llm::LlmClient::new("http://localhost:0/v1", ""),
            "test-model".to_string(),
            Vec::new(),
            String::new(),
            10,
            crate::tools::ToolRegistry::new(),
            None,
            "auto",
            bus.clone(),
            npc_core::config::AffectConfig::default(),
            true,
            true,
        );
        let mut rx = bus.subscribe();

        assert!(
            engine.chat_with_context("こんにちは", None, Some("req-7")).await.is_err(),
            "an unreachable endpoint must surface as an error, not a reply"
        );

        let errors: Vec<npc_core::BusMessage> = std::iter::from_fn(|| rx.try_recv().ok())
            .filter(|m| m.env.r#type == msg::CHAT_ERROR)
            .collect();
        assert_eq!(errors.len(), 1, "exactly one failure report per failed turn");
        assert_eq!(errors[0].env.payload["request_id"], "req-7");
        assert!(
            errors[0].env.payload["message"].as_str().is_some_and(|m| !m.is_empty()),
            "the report has to say something about what went wrong"
        );
    }

    /// The mirror of the above: a failed turn nobody requested has no client
    /// to release, so it stays a log line rather than becoming a frame
    /// broadcast at every connected tab.
    #[tokio::test]
    async fn a_failed_self_initiated_turn_reports_nothing() {
        let bus = npc_core::Bus::new();
        let engine = ChatEngine::new(
            npc_llm::LlmClient::new("http://localhost:0/v1", ""),
            "test-model".to_string(),
            Vec::new(),
            String::new(),
            10,
            crate::tools::ToolRegistry::new(),
            None,
            "auto",
            bus.clone(),
            npc_core::config::AffectConfig::default(),
            true,
            true,
        );
        let mut rx = bus.subscribe();

        assert!(engine.chat_with_context("こんにちは", None, None).await.is_err());

        assert!(
            std::iter::from_fn(|| rx.try_recv().ok()).all(|m| m.env.r#type != msg::CHAT_ERROR),
            "no requester means nobody to report to"
        );
    }
}
