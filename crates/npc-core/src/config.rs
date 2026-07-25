//! Application configuration: a single JSON document (`config.json` by
//! default) with sections mirroring the config each Go agent-* service used
//! to load independently. `.env` (via `dotenvy`) is loaded first, then a
//! small set of environment variables can override the corresponding JSON
//! fields — this matches how the OpenAI-style API key was historically kept
//! out of the checked-in config file.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Top-level configuration document.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Config {
    /// Language every LLM-facing module is told to answer in: `auto`, `ja`,
    /// `en`, or `zh`. `auto` (also empty / anything unrecognized) injects
    /// nothing, leaving each prompt's own wording in charge — that's the
    /// pre-existing behavior, so an old config.json keeps working unchanged.
    /// See [`language_instruction`].
    #[serde(default = "default_language")]
    pub language: String,
    #[serde(default)]
    pub api: ApiConfig,
    #[serde(default)]
    pub tts: TtsConfig,
    #[serde(default)]
    pub stt: SttConfig,
    #[serde(default)]
    pub talk: TalkConfig,
    #[serde(default)]
    pub memory: MemoryConfig,
    #[serde(default)]
    pub vision: VisionConfig,
    #[serde(default)]
    pub speech: SpeechConfig,
    #[serde(default)]
    pub action: ActionConfig,
    #[serde(default)]
    pub vrc: VrcConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub translation: TranslationConfig,
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default)]
    pub character: CharacterConfig,
    #[serde(default)]
    pub mist: MistConfig,
}

// ---------------------------------------------------------------------
// language
// ---------------------------------------------------------------------

fn default_language() -> String {
    "auto".to_string()
}

/// The one-line instruction appended to (or pushed alongside) every LLM
/// system prompt so replies come back in [`Config::language`].
///
/// `None` for `auto` — and for the empty string `Config::default()` leaves
/// behind, plus any unrecognized value — so an unset or hand-broken language
/// degrades to "prompt decides", never to a hard error.
///
/// Written in English on purpose: instruction-following on a "reply in X"
/// directive is more reliable in English across the small local models this
/// app targets, and the target language is named in both English and its own
/// script so the model can't mistake which one is meant.
pub fn language_instruction(language: &str) -> Option<&'static str> {
    match language.trim().to_ascii_lowercase().as_str() {
        "ja" => Some("Always write your reply in Japanese (日本語)."),
        "en" => Some("Always write your reply in English."),
        "zh" => Some("Always write your reply in Simplified Chinese (简体中文)."),
        _ => None,
    }
}

/// `prompt` with [`language_instruction`] appended on its own line. Used by
/// the modules whose prompt is a single string (vision, memory, action);
/// npc-talk pushes the instruction as a separate system message instead.
pub fn with_language_instruction(prompt: &str, language: &str) -> String {
    match language_instruction(language) {
        Some(instruction) if !prompt.trim().is_empty() => format!("{prompt}\n\n{instruction}"),
        Some(instruction) => instruction.to_string(),
        None => prompt.to_string(),
    }
}

// ---------------------------------------------------------------------
// api
// ---------------------------------------------------------------------

fn default_api_base_url() -> String {
    "http://localhost:11434/v1".to_string()
}
fn default_reasoning_effort() -> String {
    "none".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApiConfig {
    #[serde(default = "default_api_base_url")]
    pub base_url: String,
    /// SECRET: never log or serialize unredacted to the web UI.
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub embedding_model: String,
    /// `none` | `minimal` | `low` | `medium` | `high`, sent on every chat
    /// request — `none` is an explicit value, not "omit the parameter"
    /// (tc-docs/drafts/llm-settings-common-v1.md §2.3). An empty string is
    /// treated as `none` by `npc_llm::LlmClient::with_reasoning_effort`.
    #[serde(default = "default_reasoning_effort")]
    pub reasoning_effort: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: default_api_base_url(),
            api_key: String::new(),
            model: String::new(),
            embedding_model: String::new(),
            reasoning_effort: default_reasoning_effort(),
        }
    }
}

// ---------------------------------------------------------------------
// tts
// ---------------------------------------------------------------------

fn default_tts_voice() -> String {
    "alloy".to_string()
}
fn default_speed() -> f32 {
    1.0
}
fn default_tts_max_len() -> u32 {
    200
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Empty falls back to `api.base_url`.
    #[serde(default)]
    pub base_url: String,
    /// Empty falls back to `api.api_key`. SECRET.
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_tts_voice")]
    pub voice: String,
    #[serde(default = "default_speed")]
    pub speed: f32,
    #[serde(default = "default_tts_max_len")]
    pub max_len: u32,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            voice: default_tts_voice(),
            speed: default_speed(),
            max_len: default_tts_max_len(),
        }
    }
}

// ---------------------------------------------------------------------
// stt
// ---------------------------------------------------------------------

fn default_stt_model() -> String {
    "whisper-1".to_string()
}
fn default_silence_duration() -> f32 {
    1.5
}
fn default_input_threshold() -> f32 {
    0.01
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SttConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Empty falls back to `api.base_url`.
    #[serde(default)]
    pub base_url: String,
    /// Empty falls back to `api.api_key`. SECRET.
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_stt_model")]
    pub model: String,
    #[serde(default = "default_silence_duration")]
    pub silence_duration: f32,
    #[serde(default = "default_input_threshold")]
    pub input_threshold: f32,
}

impl Default for SttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            api_key: String::new(),
            model: default_stt_model(),
            silence_duration: default_silence_duration(),
            input_threshold: default_input_threshold(),
        }
    }
}

// ---------------------------------------------------------------------
// talk
// ---------------------------------------------------------------------

fn default_true() -> bool {
    true
}
fn default_history_size() -> u32 {
    20
}

/// A named prompt template. Content may reference `{{session_meta}}`,
/// `{{short_term_memory}}`, `{{long_term_memory}}`, and `{{persona}}`
/// placeholders, filled in by npc-talk at request time.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptEntry {
    pub name: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TalkConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_history_size")]
    pub history_size: u32,
    #[serde(default)]
    pub prompts: Vec<PromptEntry>,
    #[serde(default)]
    pub filter_prompt_name: String,
}

impl Default for TalkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            history_size: default_history_size(),
            prompts: Vec::new(),
            filter_prompt_name: String::new(),
        }
    }
}

// ---------------------------------------------------------------------
// memory
// ---------------------------------------------------------------------

fn default_idle_timeout_minutes() -> u32 {
    5
}
fn default_top_k() -> u32 {
    5
}
fn default_threshold() -> f32 {
    0.1
}
fn default_chunk_size() -> u32 {
    512
}
fn default_chunk_overlap() -> u32 {
    64
}
fn default_summarize_prompt() -> String {
    "以下の会話を短く要約してください。".to_string()
}
fn default_consolidate_prompt() -> String {
    "以下の記憶をまとめ、重複を除いて簡潔に整理してください。".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MemoryConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_idle_timeout_minutes")]
    pub idle_timeout_minutes: u32,
    #[serde(default = "default_top_k")]
    pub top_k: u32,
    #[serde(default = "default_threshold")]
    pub threshold: f32,
    #[serde(default = "default_chunk_size")]
    pub chunk_size: u32,
    #[serde(default = "default_chunk_overlap")]
    pub chunk_overlap: u32,
    /// LLM prompt used to summarize a conversation into short-term memory.
    #[serde(default = "default_summarize_prompt")]
    pub summarize_prompt: String,
    /// LLM prompt used to consolidate short-term memories into long-term
    /// memory.
    #[serde(default = "default_consolidate_prompt")]
    pub consolidate_prompt: String,
}

impl Default for MemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            idle_timeout_minutes: default_idle_timeout_minutes(),
            top_k: default_top_k(),
            threshold: default_threshold(),
            chunk_size: default_chunk_size(),
            chunk_overlap: default_chunk_overlap(),
            summarize_prompt: default_summarize_prompt(),
            consolidate_prompt: default_consolidate_prompt(),
        }
    }
}

// ---------------------------------------------------------------------
// vision
// ---------------------------------------------------------------------

fn default_vision_interval() -> f32 {
    5.0
}
fn default_capture_mode() -> String {
    "entire".to_string()
}
fn default_vision_system_prompt() -> String {
    "画面のスクリーンショットを簡潔に日本語で説明してください。".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VisionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    /// SECRET.
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub model: String,
    #[serde(default = "default_vision_interval")]
    pub interval_seconds: f32,
    #[serde(default = "default_capture_mode")]
    pub capture_mode: String,
    #[serde(default)]
    pub target_window_title: String,
    #[serde(default = "default_vision_system_prompt")]
    pub system_prompt: String,
    #[serde(default)]
    pub debug_save: bool,
}

impl Default for VisionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: String::new(),
            api_key: String::new(),
            model: String::new(),
            interval_seconds: default_vision_interval(),
            capture_mode: default_capture_mode(),
            target_window_title: String::new(),
            system_prompt: default_vision_system_prompt(),
            debug_save: false,
        }
    }
}

// ---------------------------------------------------------------------
// speech
// ---------------------------------------------------------------------

fn default_input_sample_rate() -> u32 {
    16000
}
fn default_output_sample_rate() -> u32 {
    44100
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SpeechConfig {
    #[serde(default)]
    pub input_device: String,
    #[serde(default)]
    pub output_device: String,
    #[serde(default = "default_input_sample_rate")]
    pub input_sample_rate: u32,
    #[serde(default = "default_output_sample_rate")]
    pub output_sample_rate: u32,
}

impl Default for SpeechConfig {
    fn default() -> Self {
        Self {
            input_device: String::new(),
            output_device: String::new(),
            input_sample_rate: default_input_sample_rate(),
            output_sample_rate: default_output_sample_rate(),
        }
    }
}

// ---------------------------------------------------------------------
// action
// ---------------------------------------------------------------------

fn default_osc_address() -> String {
    "127.0.0.1:9000".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LocationConfig {
    pub name: String,
    #[serde(default)]
    pub x: f64,
    #[serde(default)]
    pub y: f64,
    #[serde(default)]
    pub heading: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WaypointConfig {
    pub location: String,
    #[serde(default)]
    pub seconds: f64,
    #[serde(default)]
    pub wait: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    pub name: String,
    #[serde(rename = "loop", default)]
    pub r#loop: bool,
    #[serde(default)]
    pub waypoints: Vec<WaypointConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ActionConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_osc_address")]
    pub osc_address: String,
    #[serde(default)]
    pub locations: Vec<LocationConfig>,
    #[serde(default)]
    pub routes: Vec<RouteConfig>,
}

impl Default for ActionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            osc_address: default_osc_address(),
            locations: Vec::new(),
            routes: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------
// vrc
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VrcConfig {
    #[serde(default)]
    pub chatbox: bool,
    #[serde(default = "default_osc_address")]
    pub osc_address: String,
}

impl Default for VrcConfig {
    fn default() -> Self {
        Self {
            chatbox: false,
            osc_address: default_osc_address(),
        }
    }
}

// ---------------------------------------------------------------------
// scheduler
// ---------------------------------------------------------------------

fn default_volume() -> f32 {
    1.0
}

/// One extra thing an announcement does when it fires, on top of speaking its
/// `text`. Ports Go `agent-scheduler`'s `announcements[].redis_actions`
/// (`{channel, type, payload}` published to Redis) — since every former
/// service now lives in this one process, the common cases are named variants
/// that publish the right bus envelope, with [`ScheduledAction::Raw`] left as
/// the literal `{topic, type, payload}` escape hatch the Go config had.
///
/// Serialized internally tagged on `kind`, e.g.
/// `{"kind": "action", "content": "原点に移動して"}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ScheduledAction {
    /// An extra spoken line (`agent:interrupt` / `tts`), independent of the
    /// announcement's own `text` — e.g. a second voice line after a chime.
    Speak {
        #[serde(default)]
        content: String,
        #[serde(default)]
        chime_file: String,
    },
    /// Natural-language instruction for npc-action's LLM action flow
    /// (`agent:action` / `action`) — the Go config's
    /// `{channel: "agent:action", type: "action", payload: "原点に移動して"}`.
    Action {
        #[serde(default)]
        content: String,
    },
    /// CLI-style command for npc-action's dispatcher (`agent:action` /
    /// `command`), e.g. `route patrol` or `go home 15`.
    Command {
        #[serde(default)]
        text: String,
    },
    /// Inject text as if it had been heard (`agent:sense` / `speech`), so
    /// npc-talk answers it in character instead of the text being read out
    /// verbatim.
    Chat {
        #[serde(default)]
        content: String,
    },
    /// Pause npc-talk / TTS playback (`agent:interrupt` / `suspend`).
    Suspend,
    /// Undo a `suspend` (`agent:interrupt` / `resume`).
    Resume,
    /// Publish an arbitrary envelope on an arbitrary topic — a direct port of
    /// a Go `redis_actions` entry, for anything the named variants above
    /// don't cover.
    Raw {
        topic: String,
        #[serde(rename = "type")]
        msg_type: String,
        #[serde(default)]
        payload: Value,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnouncementConfig {
    /// "HH:MM" or "HH:MM:SS".
    pub time: String,
    pub text: String,
    #[serde(default)]
    pub chime_file: String,
    #[serde(default = "default_volume")]
    pub volume: f32,
    /// Extra bus messages published when this announcement fires, in order.
    /// An announcement with an empty `text` and a non-empty `actions` list is
    /// a perfectly good "do something at 17:00" entry — that's exactly what
    /// the Go original's text-less `redis_actions` entries were.
    #[serde(default)]
    pub actions: Vec<ScheduledAction>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SchedulerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub announcements: Vec<AnnouncementConfig>,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            announcements: Vec::new(),
        }
    }
}

// ---------------------------------------------------------------------
// translation (simultaneous interpretation)
// ---------------------------------------------------------------------

fn default_translation_mode() -> String {
    "off".to_string()
}
fn default_source_language() -> String {
    "日本語".to_string()
}
fn default_target_language() -> String {
    "英語".to_string()
}
fn default_context_size() -> u32 {
    3
}

/// What [`TranslationConfig::mode`] resolves to. Anything unrecognized (an
/// old config, a typo) is [`TranslationMode::Off`], so a broken value can
/// never silently swallow the NPC's replies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationMode {
    /// Nothing is translated.
    Off,
    /// Simultaneous interpretation: heard speech is translated, and npc-talk
    /// does *not* answer it — the NPC is a interpreter, not a conversation
    /// partner. Ports agent-speech's `translation.enabled`.
    Interpret,
    /// Normal conversation plus subtitles: both what was heard and what the
    /// NPC answered get translated. Ports agent-speech's
    /// `translation.agent_enabled`.
    Assist,
}

impl TranslationMode {
    pub fn is_off(self) -> bool {
        self == TranslationMode::Off
    }

    /// The config-file spelling, so logs and the startup banner say the same
    /// word the user typed.
    pub fn as_str(self) -> &'static str {
        match self {
            TranslationMode::Off => "off",
            TranslationMode::Interpret => "interpret",
            TranslationMode::Assist => "assist",
        }
    }
}

impl std::fmt::Display for TranslationMode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Simultaneous interpretation settings, ported from Go `agent-speech`'s
/// `translation` config section (`internal/config/config.go`'s
/// `TranslationConfig` + `internal/app/app.go`'s `handleTranslation`). The Go
/// original's mutually-exclusive `enabled` / `agent_enabled` pair is a single
/// [`mode`](TranslationConfig::mode) here.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TranslationConfig {
    /// `off` | `interpret` | `assist` — see [`TranslationMode`].
    #[serde(default = "default_translation_mode")]
    pub mode: String,
    /// The language the speaker is expected to use; the target
    /// `auto_reverse` translates *back* into when someone answers in a
    /// different language.
    #[serde(default = "default_source_language")]
    pub source_language: String,
    /// Primary target language. These are free-form labels handed straight to
    /// the LLM ("英語", "中国語", "台湾華語", "English", …), matching the Go
    /// original's `config.Lang*` constants.
    #[serde(default = "default_target_language")]
    pub target_language: String,
    /// Optional second target language; empty means single-target.
    #[serde(default)]
    pub target_language_2: String,
    /// How many previous utterances are handed to the LLM as context.
    #[serde(default = "default_context_size")]
    pub context_size: u32,
    /// When the detected language of an utterance isn't `source_language`,
    /// translate it back into `source_language` instead of into the targets —
    /// this is what makes a two-way conversation work with one setting.
    #[serde(default = "default_true")]
    pub auto_reverse: bool,
    /// Also send translations to the VRChat chatbox over OSC (uses
    /// `vrc.osc_address`, independent of `vrc.chatbox`).
    #[serde(default)]
    pub chatbox: bool,
    /// Model override for translation requests; empty falls back to
    /// `api.model`.
    #[serde(default)]
    pub model: String,
}

impl Default for TranslationConfig {
    fn default() -> Self {
        Self {
            mode: default_translation_mode(),
            source_language: default_source_language(),
            target_language: default_target_language(),
            target_language_2: String::new(),
            context_size: default_context_size(),
            auto_reverse: true,
            chatbox: false,
            model: String::new(),
        }
    }
}

impl TranslationConfig {
    pub fn mode(&self) -> TranslationMode {
        match self.mode.trim().to_ascii_lowercase().as_str() {
            "interpret" => TranslationMode::Interpret,
            "assist" => TranslationMode::Assist,
            _ => TranslationMode::Off,
        }
    }

    /// Whether npc-talk should stay silent about what it hears (see
    /// [`TranslationMode::Interpret`]).
    pub fn suppresses_chat(&self) -> bool {
        self.mode() == TranslationMode::Interpret
    }

    /// The configured targets in order, de-duplicated and without blanks.
    /// Ports Go `targetLanguages`.
    pub fn target_languages(&self) -> Vec<String> {
        let mut langs = Vec::new();
        for lang in [&self.target_language, &self.target_language_2] {
            let lang = lang.trim();
            if !lang.is_empty() && !langs.iter().any(|l: &String| l == lang) {
                langs.push(lang.to_string());
            }
        }
        langs
    }
}

// ---------------------------------------------------------------------
// server
// ---------------------------------------------------------------------

fn default_server_addr() -> String {
    "127.0.0.1:47950".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ServerConfig {
    #[serde(default = "default_server_addr")]
    pub addr: String,
    #[serde(default = "default_true")]
    pub auto_open: bool,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            addr: default_server_addr(),
            auto_open: true,
        }
    }
}

// ---------------------------------------------------------------------
// character
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CharacterConfig {
    #[serde(default)]
    pub active_id: String,
}

// ---------------------------------------------------------------------
// mist
// ---------------------------------------------------------------------

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MistConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub signaling_url: String,
    #[serde(default)]
    pub room_id: String,
}

// ---------------------------------------------------------------------
// loading
// ---------------------------------------------------------------------

impl Config {
    /// Resolve the config path: the explicit `--config` path if given,
    /// otherwise `./config.json`.
    pub fn resolve_path(explicit: Option<&Path>) -> PathBuf {
        explicit
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("config.json"))
    }

    /// Load configuration from `path` (or `./config.json` if `path` is
    /// `None`). A missing file is not an error — it just yields defaults.
    /// `.env` is loaded first (via `dotenvy`), then a fixed set of
    /// environment variables override the corresponding JSON fields.
    pub fn load(path: Option<&Path>) -> anyhow::Result<Config> {
        // Best-effort: a missing .env file is fine.
        let _ = dotenvy::dotenv();

        let resolved = Self::resolve_path(path);
        let mut config = if resolved.exists() {
            let data = std::fs::read_to_string(&resolved).map_err(|e| {
                anyhow::anyhow!("failed to read config file {}: {e}", resolved.display())
            })?;
            serde_json::from_str::<Config>(&data).map_err(|e| {
                anyhow::anyhow!("failed to parse config file {}: {e}", resolved.display())
            })?
        } else {
            Config::default()
        };

        config.apply_env_overrides();
        Ok(config)
    }

    fn apply_env_overrides(&mut self) {
        if let Ok(v) = std::env::var("OPENAI_API_KEY") {
            self.api.api_key = v;
        }
        if let Ok(v) = std::env::var("OPENAI_API_BASE_URL") {
            self.api.base_url = v;
        }
        if let Ok(v) = std::env::var("OPENAI_MODEL") {
            self.api.model = v;
        }
        if let Ok(v) = std::env::var("OPENAI_EMBEDDING_MODEL") {
            self.api.embedding_model = v;
        }
        if let Ok(v) = std::env::var("OPENAI_REASONING_EFFORT") {
            self.api.reasoning_effort = v;
        }
        if let Ok(v) = std::env::var("TTS_BASE_URL") {
            self.tts.base_url = v;
        }
        if let Ok(v) = std::env::var("TTS_API_KEY") {
            self.tts.api_key = v;
        }
        if let Ok(v) = std::env::var("STT_BASE_URL") {
            self.stt.base_url = v;
        }
        if let Ok(v) = std::env::var("STT_API_KEY") {
            self.stt.api_key = v;
        }
    }

    /// Serialize this config to JSON with every `api_key` field masked, for
    /// safe display in the web UI.
    pub fn redacted_json(&self) -> anyhow::Result<Value> {
        let mut value = serde_json::to_value(self)?;
        redact_api_keys(&mut value);
        Ok(value)
    }
}

fn redact_api_keys(value: &mut Value) {
    match value {
        Value::Object(map) => {
            for (key, v) in map.iter_mut() {
                if key == "api_key" {
                    if let Value::String(s) = v {
                        if !s.is_empty() {
                            *s = "***".to_string();
                        }
                    }
                } else {
                    redact_api_keys(v);
                }
            }
        }
        Value::Array(arr) => {
            for v in arr.iter_mut() {
                redact_api_keys(v);
            }
        }
        _ => {}
    }
}

/// The app's data directory, `~/.tc-npc/`. Created on demand by callers that
/// need it to exist.
pub fn data_dir() -> PathBuf {
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".tc-npc")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_round_trip() {
        let config = Config::default();
        assert_eq!(config.api.base_url, "http://localhost:11434/v1");
        assert!(config.talk.enabled);
        assert_eq!(config.server.addr, "127.0.0.1:47950");
    }

    #[test]
    fn parses_partial_json() {
        let json = r#"{"api": {"model": "gpt-4o-mini"}}"#;
        let config: Config = serde_json::from_str(json).unwrap();
        assert_eq!(config.api.model, "gpt-4o-mini");
        // Untouched field falls back to its default.
        assert_eq!(config.api.base_url, "http://localhost:11434/v1");
        assert_eq!(config.api.reasoning_effort, "none");
    }

    #[test]
    fn language_defaults_to_auto_and_injects_nothing() {
        // A config file written before `language` existed.
        let config: Config = serde_json::from_str(r#"{"api": {"model": "m"}}"#).unwrap();
        assert_eq!(config.language, "auto");
        assert_eq!(language_instruction(&config.language), None);
        // `Config::default()` leaves an empty string (derived Default doesn't
        // run serde's `default_language`); it must behave like `auto` too.
        assert_eq!(language_instruction(&Config::default().language), None);
    }

    #[test]
    fn language_instruction_covers_the_three_supported_languages() {
        assert!(language_instruction("ja").unwrap().contains("Japanese"));
        assert!(language_instruction("EN").unwrap().contains("English"));
        assert!(language_instruction(" zh ").unwrap().contains("Chinese"));
        assert_eq!(language_instruction("klingon"), None);
    }

    #[test]
    fn with_language_instruction_appends_or_passes_through() {
        assert_eq!(with_language_instruction("Describe it.", "auto"), "Describe it.");
        assert_eq!(
            with_language_instruction("Describe it.", "en"),
            "Describe it.\n\nAlways write your reply in English."
        );
        // An empty prompt becomes the instruction alone, not a blank-line prefix.
        assert_eq!(
            with_language_instruction("  ", "en"),
            "Always write your reply in English."
        );
    }

    #[test]
    fn announcement_actions_default_to_empty_and_parse_by_kind() {
        // An announcement written before `actions` existed still loads.
        let ann: AnnouncementConfig =
            serde_json::from_str(r#"{"time": "09:00", "text": "hi"}"#).unwrap();
        assert!(ann.actions.is_empty());

        let ann: AnnouncementConfig = serde_json::from_str(
            r#"{
                "time": "17:00",
                "text": "",
                "actions": [
                    {"kind": "action", "content": "原点に移動して"},
                    {"kind": "command", "text": "route patrol"},
                    {"kind": "chat", "content": "今日の予定は?"},
                    {"kind": "speak", "content": "こんにちは", "chime_file": "c.wav"},
                    {"kind": "suspend"},
                    {"kind": "resume"},
                    {"kind": "raw", "topic": "agent:interrupt", "type": "resume", "payload": {}}
                ]
            }"#,
        )
        .unwrap();
        assert_eq!(ann.actions.len(), 7);
        assert_eq!(
            ann.actions[0],
            ScheduledAction::Action {
                content: "原点に移動して".to_string()
            }
        );
        assert_eq!(ann.actions[5], ScheduledAction::Resume);
        assert_eq!(
            ann.actions[6],
            ScheduledAction::Raw {
                topic: "agent:interrupt".to_string(),
                msg_type: "resume".to_string(),
                payload: serde_json::json!({}),
            }
        );
    }

    #[test]
    fn scheduled_action_round_trips_through_json() {
        let action = ScheduledAction::Command {
            text: "go home".to_string(),
        };
        let json = serde_json::to_value(&action).unwrap();
        assert_eq!(json["kind"], "command");
        assert_eq!(json["text"], "go home");
        assert_eq!(serde_json::from_value::<ScheduledAction>(json).unwrap(), action);
    }

    #[test]
    fn translation_defaults_to_off_for_an_old_config() {
        let config: Config = serde_json::from_str(r#"{"api": {"model": "m"}}"#).unwrap();
        assert_eq!(config.translation.mode(), TranslationMode::Off);
        assert!(!config.translation.suppresses_chat());
        // `Config::default()`'s derived Default leaves `mode` empty, which
        // must behave like "off" too.
        assert_eq!(Config::default().translation.mode(), TranslationMode::Off);
    }

    #[test]
    fn translation_mode_parses_leniently() {
        let mut translation = TranslationConfig::default();
        translation.mode = " Interpret ".to_string();
        assert_eq!(translation.mode(), TranslationMode::Interpret);
        assert!(translation.suppresses_chat());
        translation.mode = "assist".to_string();
        assert_eq!(translation.mode(), TranslationMode::Assist);
        assert!(!translation.suppresses_chat());
        translation.mode = "nonsense".to_string();
        assert_eq!(translation.mode(), TranslationMode::Off);
    }

    #[test]
    fn target_languages_drop_blanks_and_duplicates() {
        let mut translation = TranslationConfig::default();
        translation.target_language = "英語".to_string();
        translation.target_language_2 = String::new();
        assert_eq!(translation.target_languages(), vec!["英語".to_string()]);

        translation.target_language_2 = "中国語".to_string();
        assert_eq!(
            translation.target_languages(),
            vec!["英語".to_string(), "中国語".to_string()]
        );

        translation.target_language_2 = " 英語 ".to_string();
        assert_eq!(translation.target_languages(), vec!["英語".to_string()]);
    }

    #[test]
    fn redacts_api_keys() {
        let mut config = Config::default();
        config.api.api_key = "sk-secret".to_string();
        let redacted = config.redacted_json().unwrap();
        assert_eq!(redacted["api"]["api_key"], "***");
    }
}
