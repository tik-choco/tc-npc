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
    pub server: ServerConfig,
    #[serde(default)]
    pub character: CharacterConfig,
    #[serde(default)]
    pub mist: MistConfig,
}

// ---------------------------------------------------------------------
// api
// ---------------------------------------------------------------------

fn default_api_base_url() -> String {
    "http://localhost:11434/v1".to_string()
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
    #[serde(default)]
    pub reasoning_effort: String,
}

impl Default for ApiConfig {
    fn default() -> Self {
        Self {
            base_url: default_api_base_url(),
            api_key: String::new(),
            model: String::new(),
            embedding_model: String::new(),
            reasoning_effort: String::new(),
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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnnouncementConfig {
    /// "HH:MM" or "HH:MM:SS".
    pub time: String,
    pub text: String,
    #[serde(default)]
    pub chime_file: String,
    #[serde(default = "default_volume")]
    pub volume: f32,
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
    }

    #[test]
    fn redacts_api_keys() {
        let mut config = Config::default();
        config.api.api_key = "sk-secret".to_string();
        let redacted = config.redacted_json().unwrap();
        assert_eq!(redacted["api"]["api_key"], "***");
    }
}
