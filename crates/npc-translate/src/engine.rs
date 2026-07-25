//! The interpretation engine: rolling context, target-language selection
//! (including auto-reverse), the LLM call itself, and publishing results.
//!
//! Faithful port of Go `agent-speech`'s `internal/app/app.go`
//! (`handleTranslation` / `handleAgentTranslation` / `composeVRCChatbox` and
//! friends), down to the prompt wording and the "Previous message:" context
//! format — different phrasing measurably changes what small local models
//! hand back, so it's deliberately unchanged.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, RwLock};

use futures_util::future::join_all;
use npc_core::config::{Config, TranslationConfig, TranslationMode};
use npc_core::Bus;
use npc_llm::{ChatMessage, ChatRequest, LlmClient};
use serde_json::json;

use crate::langdetect::detect_language;

/// Bus topic UI-only updates go out on (same one npc-action/npc-vision use
/// for their web-UI feeds); npc-server forwards these to WS clients.
pub const UI_TOPIC: &str = "npc:ui";
/// Envelope type for one interpretation update. Two shapes share it, keyed by
/// `lang`:
///
/// * `lang: ""` — the original line, published as soon as it's heard so the
///   UI can show it while the translations are still in flight.
/// * `lang: "英語"` — one finished translation for that entry.
///
/// Both carry the same `id`, so the client upserts by id and fills in
/// `translations[lang]` as each arrives (ports the Go web UI's
/// `TranslationMessage` grouping).
pub const UI_MSG_TRANSLATION: &str = "translation";

/// Who said the thing being translated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    /// Heard speech (`agent:sense` / `speech`).
    User,
    /// The NPC's own reply (`agent:chat` / `chat_response`).
    Agent,
}

impl Source {
    fn as_str(self) -> &'static str {
        match self {
            Source::User => "user",
            Source::Agent => "agent",
        }
    }
}

pub struct TranslationEngine {
    bus: Bus,
    llm: LlmClient,
    /// `api.model`, used when `translation.model` is empty.
    fallback_model: String,
    /// `vrc.osc_address` — where chatbox subtitles go.
    osc_address: String,
    /// Hot-reloaded from `npc:config`; everything else above is fixed for the
    /// process lifetime, matching how the other modules treat connection
    /// settings.
    settings: RwLock<TranslationConfig>,
    user_history: Mutex<Vec<String>>,
    agent_history: Mutex<Vec<String>>,
    seq: AtomicU64,
}

impl TranslationEngine {
    pub fn new(bus: Bus, config: &Config) -> Self {
        let llm = LlmClient::new(config.api.base_url.clone(), config.api.api_key.clone())
            .with_reasoning_effort(config.api.reasoning_effort.clone());
        Self {
            bus,
            llm,
            fallback_model: config.api.model.clone(),
            osc_address: config.vrc.osc_address.clone(),
            settings: RwLock::new(config.translation.clone()),
            user_history: Mutex::new(Vec::new()),
            agent_history: Mutex::new(Vec::new()),
            seq: AtomicU64::new(0),
        }
    }

    pub fn mode(&self) -> TranslationMode {
        self.settings.read().unwrap().mode()
    }

    /// Adopt the `translation` section of a freshly saved config. Endpoint
    /// and model *connection* settings (`api.*`) are not re-read — those
    /// still need a restart, as everywhere else in this app.
    pub fn update_settings(&self, config: &Config) {
        *self.settings.write().unwrap() = config.translation.clone();
    }

    /// Translate one utterance into every configured target language,
    /// publishing each result as it lands.
    pub async fn translate(&self, source: Source, text: String) {
        let settings = self.settings.read().unwrap().clone();

        let context = {
            let history = match source {
                Source::User => &self.user_history,
                Source::Agent => &self.agent_history,
            };
            let mut history = history.lock().unwrap();
            push_history(&mut history, &text, settings.context_size as usize);
            build_context(&history)
        };

        let (langs, reversed) = resolve_targets(&settings, source, &text);
        if langs.is_empty() {
            tracing::debug!("npc-translate: no target languages configured, skipping");
            return;
        }

        let id = format!("t{}", self.seq.fetch_add(1, Ordering::SeqCst) + 1);

        // The original line first, so the UI shows what was heard without
        // waiting on the LLM.
        self.publish(&id, source, &text, "", "", reversed);

        let model = if settings.model.trim().is_empty() {
            self.fallback_model.clone()
        } else {
            settings.model.clone()
        };

        // Results accumulate as they arrive so each chatbox update is a
        // fuller version of the same text (ports `composeVRCChatbox`).
        let results: Mutex<HashMap<String, String>> = Mutex::new(HashMap::new());

        join_all(langs.iter().map(|lang| {
            let prompt = build_prompt(lang, &context, &text);
            let model = model.clone();
            let id = &id;
            let text = &text;
            let langs = &langs;
            let results = &results;
            let settings = &settings;
            async move {
                let translated = match self.request(&model, prompt).await {
                    Ok(t) => t,
                    Err(err) => {
                        tracing::error!(error = %err, lang = %lang, "npc-translate: translation failed");
                        return;
                    }
                };
                if translated.is_empty() {
                    return;
                }

                self.publish(id, source, text, lang, &translated, reversed);

                if settings.chatbox {
                    let chatbox = {
                        let mut results = results.lock().unwrap();
                        results.insert(lang.to_string(), translated.clone());
                        compose_chatbox(text, langs, &results)
                    };
                    let addr = self.osc_address.clone();
                    tokio::task::spawn_blocking(move || npc_core::osc::send_chatbox(&addr, &chatbox));
                }
            }
        }))
        .await;
    }

    async fn request(&self, model: &str, prompt: String) -> anyhow::Result<String> {
        let req = ChatRequest::new(model.to_string(), vec![ChatMessage::user(prompt)]);
        let resp = self.llm.chat(req).await?;
        Ok(resp.content().unwrap_or_default().trim().to_string())
    }

    fn publish(&self, id: &str, source: Source, original: &str, lang: &str, text: &str, reversed: bool) {
        self.bus.publish(
            UI_TOPIC,
            UI_MSG_TRANSLATION,
            json!({
                "id": id,
                "source": source.as_str(),
                "original": original,
                "lang": lang,
                "text": text,
                "reversed": reversed,
                "ts": chrono::Utc::now().timestamp_millis(),
            }),
        );
    }
}

/// Append `text` and keep only the last `context_size` entries. A
/// `context_size` of 0 means "no context", which still keeps the current
/// utterance (the Go original's slice arithmetic works out the same way).
fn push_history(history: &mut Vec<String>, text: &str, context_size: usize) {
    history.push(text.to_string());
    let keep = context_size.max(1);
    if history.len() > keep {
        history.drain(0..history.len() - keep);
    }
}

/// The `Context for translation:` block prefixed to the prompt — every
/// remembered utterance *except* the current one (which the prompt itself
/// carries as the text to translate).
fn build_context(history: &[String]) -> String {
    if history.len() < 2 {
        return String::new();
    }
    let lines: Vec<String> = history[..history.len() - 1]
        .iter()
        .map(|h| format!("Previous message: {h}"))
        .collect();
    format!("Context for translation:\n{}\n\n", lines.join("\n"))
}

fn build_prompt(lang: &str, context: &str, text: &str) -> String {
    format!(
        "Translate the following text to {lang} accurately and concisely, using the provided \
         context for reference but translating ONLY the final text. Return ONLY the translated \
         text.\n\n{context}Text to translate: {text}"
    )
}

/// The languages `text` should be translated into, plus whether that decision
/// was an auto-reverse.
///
/// Auto-reverse (heard speech only): when the speaker answers *in* one of the
/// target languages, they're replying to a translation — so translate back
/// into `source_language` rather than into the language it's already in. It
/// deliberately only triggers when the detected language is one of the
/// configured targets, so an unrelated third language still goes to the
/// normal targets.
fn resolve_targets(settings: &TranslationConfig, source: Source, text: &str) -> (Vec<String>, bool) {
    let langs = settings.target_languages();
    if source == Source::Agent || !settings.auto_reverse || langs.is_empty() {
        return (langs, false);
    }

    let Some(detected) = detect_language(text) else {
        return (langs, false);
    };
    if detected == settings.source_language.trim() {
        return (langs, false);
    }
    if !langs.iter().any(|l| l == detected) {
        return (langs, false);
    }

    let back = settings.source_language.trim();
    if back.is_empty() {
        return (langs, false);
    }
    (vec![back.to_string()], true)
}

/// The chatbox text for an entry: the original followed by whichever
/// translations have arrived, in configured language order.
fn compose_chatbox(original: &str, langs: &[String], results: &HashMap<String, String>) -> String {
    let mut lines: Vec<&str> = Vec::with_capacity(langs.len() + 1);
    let original = original.trim();
    if !original.is_empty() {
        lines.push(original);
    }
    for lang in langs {
        if let Some(v) = results.get(lang) {
            lines.push(v);
        }
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn settings() -> TranslationConfig {
        TranslationConfig {
            mode: "interpret".to_string(),
            source_language: "日本語".to_string(),
            target_language: "英語".to_string(),
            target_language_2: String::new(),
            context_size: 3,
            auto_reverse: true,
            chatbox: false,
            model: String::new(),
        }
    }

    #[test]
    fn history_keeps_only_the_configured_window() {
        let mut history = Vec::new();
        for i in 0..5 {
            push_history(&mut history, &format!("m{i}"), 3);
        }
        assert_eq!(history, vec!["m2".to_string(), "m3".to_string(), "m4".to_string()]);
    }

    #[test]
    fn zero_context_size_still_keeps_the_current_utterance() {
        let mut history = Vec::new();
        push_history(&mut history, "a", 0);
        push_history(&mut history, "b", 0);
        assert_eq!(history, vec!["b".to_string()]);
        assert_eq!(build_context(&history), "");
    }

    #[test]
    fn context_excludes_the_current_utterance() {
        let history = vec!["one".to_string(), "two".to_string(), "three".to_string()];
        assert_eq!(
            build_context(&history),
            "Context for translation:\nPrevious message: one\nPrevious message: two\n\n"
        );
    }

    #[test]
    fn context_is_empty_for_the_first_utterance() {
        assert_eq!(build_context(&["only".to_string()]), "");
        assert_eq!(build_context(&[]), "");
    }

    #[test]
    fn prompt_names_the_target_language_and_carries_context() {
        let prompt = build_prompt("英語", "Context for translation:\nPrevious message: x\n\n", "こんにちは");
        assert!(prompt.starts_with("Translate the following text to 英語 accurately"));
        assert!(prompt.contains("Previous message: x"));
        assert!(prompt.ends_with("Text to translate: こんにちは"));
    }

    #[test]
    fn targets_are_the_configured_languages_by_default() {
        let mut s = settings();
        s.target_language_2 = "中国語".to_string();
        let (langs, reversed) = resolve_targets(&s, Source::User, "こんにちは");
        assert_eq!(langs, vec!["英語".to_string(), "中国語".to_string()]);
        assert!(!reversed);
    }

    #[test]
    fn auto_reverse_flips_a_reply_in_a_target_language_back_to_the_source() {
        let (langs, reversed) = resolve_targets(&settings(), Source::User, "hello there");
        assert_eq!(langs, vec!["日本語".to_string()]);
        assert!(reversed);
    }

    #[test]
    fn auto_reverse_ignores_a_language_that_is_not_a_target() {
        let mut s = settings();
        s.target_language = "中国語".to_string();
        // English is detected but isn't a target, so the normal targets win.
        let (langs, reversed) = resolve_targets(&s, Source::User, "hello there");
        assert_eq!(langs, vec!["中国語".to_string()]);
        assert!(!reversed);
    }

    #[test]
    fn auto_reverse_can_be_switched_off() {
        let mut s = settings();
        s.auto_reverse = false;
        let (langs, reversed) = resolve_targets(&s, Source::User, "hello there");
        assert_eq!(langs, vec!["英語".to_string()]);
        assert!(!reversed);
    }

    #[test]
    fn agent_replies_never_auto_reverse() {
        // The NPC answers in its own language; its reply always goes to the
        // configured targets.
        let (langs, reversed) = resolve_targets(&settings(), Source::Agent, "hello there");
        assert_eq!(langs, vec!["英語".to_string()]);
        assert!(!reversed);
    }

    #[test]
    fn chatbox_lists_the_original_then_arrived_translations_in_order() {
        let langs = vec!["英語".to_string(), "中国語".to_string()];
        let mut results = HashMap::new();
        results.insert("中国語".to_string(), "你好".to_string());
        assert_eq!(compose_chatbox("こんにちは", &langs, &results), "こんにちは\n你好");

        results.insert("英語".to_string(), "Hello".to_string());
        assert_eq!(
            compose_chatbox("こんにちは", &langs, &results),
            "こんにちは\nHello\n你好"
        );
    }
}
