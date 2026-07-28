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
    /// 接続先(provider)一覧。「どこに繋ぐか」だけを持つ。空の場合は
    /// `migrate_llm_config` が起動時に `api.*` から1件だけ組み立てる。
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    /// モデルプリセット一覧。「どう呼ぶか」だけを持つ。
    #[serde(default)]
    pub presets: Vec<PresetConfig>,
    /// 各タスクの `preset_id` が空のときに使う既定 preset の id。
    #[serde(default)]
    pub default_preset_id: String,
}

// ---------------------------------------------------------------------
// providers / presets
// ---------------------------------------------------------------------

/// 接続先。「どこに繋ぐか」だけを持つ(tc-docs/drafts/llm-settings-common-v1.md
/// §2.1)。`id` は設定 UI が生成する安定 id(例: `"p1"`)で、`presets[].provider_id`
/// から参照される。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProviderConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub base_url: String,
    /// SECRET: never log or serialize unredacted to the web UI.
    #[serde(default)]
    pub api_key: String,
}

/// モデルプリセット。「どう呼ぶか」だけを持つ(tc-docs/drafts/llm-settings-common-v1.md
/// §2.1)。各タスクセクションはこの `id` だけを持つ。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PresetConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub label: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub model: String,
    /// `""` = `api.reasoning_effort` を継承。
    #[serde(default)]
    pub reasoning_effort: String,
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
    /// 使用する preset の id。空なら `default_preset_id` に追従。
    #[serde(default)]
    pub preset_id: String,
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
            preset_id: String::new(),
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
/// While TTS is playing, the mic has to clear `input_threshold` times this
/// factor to count as the user talking over the agent. Above 1.0 on purpose:
/// with an open mic and speakers the mic hears the agent's own voice, and a
/// factor of 1.0 would make every reply interrupt itself.
fn default_barge_in_factor() -> f32 {
    2.5
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
    /// Let the user talk over a playing TTS reply: the mic stays live during
    /// playback and crossing [`SttConfig::barge_in_factor`] × `input_threshold`
    /// stops the clip so the interruption is heard (ports agent-speech's
    /// `streamVolume` barge-in). Off restores the previous behaviour, where
    /// the mic is muted for the whole clip.
    #[serde(default = "default_true")]
    pub barge_in: bool,
    /// Multiplier on `input_threshold` that mic level must exceed *while TTS
    /// is playing* to trigger barge-in. See [`default_barge_in_factor`].
    #[serde(default = "default_barge_in_factor")]
    pub barge_in_factor: f32,
    /// 使用する preset の id。空なら `default_preset_id` に追従。
    #[serde(default)]
    pub preset_id: String,
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
            barge_in: true,
            barge_in_factor: default_barge_in_factor(),
            preset_id: String::new(),
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
/// `{{short_term_memory}}`, `{{long_term_memory}}`, `{{person_memory}}`, and
/// `{{persona}}` placeholders, filled in by npc-talk at request time.
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
    #[serde(default)]
    pub affect: AffectConfig,
    /// 使用する preset の id。空なら `default_preset_id` に追従。
    #[serde(default)]
    pub preset_id: String,
}

impl Default for TalkConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            history_size: default_history_size(),
            prompts: Vec::new(),
            filter_prompt_name: String::new(),
            affect: AffectConfig::default(),
            preset_id: String::new(),
        }
    }
}

/// Emotion-drive model settings (npc-talk's `AffectState`, ported from
/// tc-assistant2's `ConversationAffectState`). See `npc-talk::affect`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AffectConfig {
    /// Track internal drive state and inject it as a system prompt.
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// When the drive state machine considers the conversation "closing"
    /// (e.g. after a farewell), answer with a fixed short reply instead of
    /// calling the LLM at all.
    #[serde(default = "default_true")]
    pub forced_closure: bool,
    /// How long the conversation partner can stay silent before the NPC
    /// treats them as having walked away: the drives settle back to baseline
    /// and the closing/invite-caution state machine clears, while what the
    /// NPC has learned about that person (familiarity, accumulated turns) is
    /// kept for when they come back. `0` disables the check entirely.
    #[serde(default = "default_absence_timeout_secs")]
    pub absence_timeout_secs: u64,
}

fn default_absence_timeout_secs() -> u64 {
    300
}

impl Default for AffectConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            forced_closure: true,
            absence_timeout_secs: default_absence_timeout_secs(),
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
    /// 人物ごとの記憶。
    #[serde(default)]
    pub people: PersonMemoryConfig,
    /// 要約・統合に使う preset の id。空なら `default_preset_id` に追従。
    #[serde(default)]
    pub preset_id: String,
    /// 埋め込みに使う preset の id。空なら `default_preset_id` に追従。
    #[serde(default)]
    pub embedding_preset_id: String,
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
            people: PersonMemoryConfig::default(),
            preset_id: String::new(),
            embedding_preset_id: String::new(),
        }
    }
}

fn default_person_max_facts() -> u32 {
    20
}
fn default_person_extract_prompt() -> String {
    "以下の会話から、登場した人物について新たに分かった事実だけを抽出してください。JSON配列で、各要素は {\"name\": \"人物名\", \"facts\": [\"事実1\", \"事実2\"]} の形式にしてください。人物が特定できない、または新しい情報がない場合は空配列 [] を返してください。説明や前置きは不要です。".to_string()
}

/// 人物ごとの記憶設定。会話・視覚から人物レコードを組み立てる `npc-memory`
/// が参照する。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersonMemoryConfig {
    /// 人物ごとの記録を行うか。既定 true。
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 会話から人物に関する事実を抽出させるLLMプロンプト。
    #[serde(default = "default_person_extract_prompt")]
    pub extract_prompt: String,
    /// 1人あたりに保持する facts の上限(超えたら古いものから捨てる)。既定 20。
    #[serde(default = "default_person_max_facts")]
    pub max_facts: u32,
    /// npc-vision の person_seen を人物レコードへ取り込むか。既定 true。
    #[serde(default = "default_true")]
    pub link_vision: bool,
}

impl Default for PersonMemoryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            extract_prompt: default_person_extract_prompt(),
            max_facts: default_person_max_facts(),
            link_vision: true,
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
    /// 画面に映った人物を検出し、`person_seen` として publish するか。既定 false。
    #[serde(default)]
    pub person_detection: bool,
    /// 使用する preset の id。空なら `default_preset_id` に追従。
    #[serde(default)]
    pub preset_id: String,
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
            person_detection: false,
            preset_id: String::new(),
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
    /// 使用する preset の id。空なら `default_preset_id` に追従。
    #[serde(default)]
    pub preset_id: String,
}

impl Default for ActionConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            osc_address: default_osc_address(),
            locations: Vec::new(),
            routes: Vec::new(),
            preset_id: String::new(),
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
    /// 使用する preset の id。空なら `default_preset_id` に追従。
    #[serde(default)]
    pub preset_id: String,
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
            preset_id: String::new(),
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
// llm task resolution (tc-docs/drafts/llm-settings-common-v1.md §2)
// ---------------------------------------------------------------------

/// Which task an LLM connection is being resolved for. Mirrors the task rows
/// described in tc-docs/drafts/llm-settings-common-v1.md §2.3/§3.2.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LlmTask {
    Talk,
    Memory,
    Embedding,
    Vision,
    Action,
    Translation,
    Tts,
    Stt,
}

/// The effective connection info for one [`LlmTask`], after resolving
/// `preset_id` -> preset -> provider (or falling back to the legacy
/// per-section fields). See [`Config::resolve_llm`].
#[derive(Clone, Debug, Default)]
pub struct ResolvedLlm {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub reasoning_effort: String,
}

impl Config {
    /// Look up a provider by id.
    pub fn provider(&self, id: &str) -> Option<&ProviderConfig> {
        self.providers.iter().find(|p| p.id == id)
    }

    /// Look up a preset by id.
    pub fn preset(&self, id: &str) -> Option<&PresetConfig> {
        self.presets.iter().find(|p| p.id == id)
    }

    /// The preset id assigned to `task`. An empty per-task `preset_id`
    /// resolves to [`Config::default_preset_id`].
    pub fn task_preset_id(&self, task: LlmTask) -> &str {
        let assigned = match task {
            LlmTask::Talk => &self.talk.preset_id,
            LlmTask::Memory => &self.memory.preset_id,
            LlmTask::Embedding => &self.memory.embedding_preset_id,
            LlmTask::Vision => &self.vision.preset_id,
            LlmTask::Action => &self.action.preset_id,
            LlmTask::Translation => &self.translation.preset_id,
            LlmTask::Tts => &self.tts.preset_id,
            LlmTask::Stt => &self.stt.preset_id,
        };
        if assigned.is_empty() {
            &self.default_preset_id
        } else {
            assigned
        }
    }

    /// The legacy (pre-provider/preset) model value for `task`, used as the
    /// last-resort fallback both when a preset's own `model` is empty and
    /// when no preset resolves at all. Must match each crate's pre-existing
    /// fallback exactly:
    /// `crates/npc-vision/src/lib.rs:82-99`,
    /// `crates/npc-translate/src/engine.rs:125-128`,
    /// `crates/npc-speech/src/lib.rs:363-367` (tts), `:173` (stt, always
    /// `stt.model` — it already carries a non-empty default).
    fn legacy_model(&self, task: LlmTask) -> String {
        match task {
            LlmTask::Talk | LlmTask::Memory | LlmTask::Action => self.api.model.clone(),
            LlmTask::Embedding => self.api.embedding_model.clone(),
            LlmTask::Vision => {
                if !self.vision.model.is_empty() {
                    self.vision.model.clone()
                } else {
                    self.api.model.clone()
                }
            }
            LlmTask::Translation => {
                if !self.translation.model.is_empty() {
                    self.translation.model.clone()
                } else {
                    self.api.model.clone()
                }
            }
            // `TtsConfig::model` empty falls back to `api.model`
            // (`synthesize_tts` in npc-speech), unlike stt below.
            LlmTask::Tts => {
                if !self.tts.model.is_empty() {
                    self.tts.model.clone()
                } else {
                    self.api.model.clone()
                }
            }
            // `SttConfig::model` has its own non-empty default
            // (`default_stt_model`) and is used as-is, with no fallback to
            // `api.model` in npc-speech.
            LlmTask::Stt => self.stt.model.clone(),
        }
    }

    /// The legacy connection info for `task` — i.e. what every crate computed
    /// before providers/presets existed. Used by [`Config::resolve_llm`] when
    /// no preset/provider pair resolves for the task.
    fn legacy_llm(&self, task: LlmTask) -> ResolvedLlm {
        let (base_url, api_key) = match task {
            LlmTask::Talk | LlmTask::Memory | LlmTask::Embedding | LlmTask::Action | LlmTask::Translation => {
                (self.api.base_url.clone(), self.api.api_key.clone())
            }
            LlmTask::Vision => (
                if !self.vision.base_url.is_empty() {
                    self.vision.base_url.clone()
                } else {
                    self.api.base_url.clone()
                },
                if !self.vision.api_key.is_empty() {
                    self.vision.api_key.clone()
                } else {
                    self.api.api_key.clone()
                },
            ),
            LlmTask::Tts => (
                if !self.tts.base_url.is_empty() {
                    self.tts.base_url.clone()
                } else {
                    self.api.base_url.clone()
                },
                if !self.tts.api_key.is_empty() {
                    self.tts.api_key.clone()
                } else {
                    self.api.api_key.clone()
                },
            ),
            LlmTask::Stt => (
                if !self.stt.base_url.is_empty() {
                    self.stt.base_url.clone()
                } else {
                    self.api.base_url.clone()
                },
                if !self.stt.api_key.is_empty() {
                    self.stt.api_key.clone()
                } else {
                    self.api.api_key.clone()
                },
            ),
        };
        ResolvedLlm {
            base_url,
            api_key,
            model: self.legacy_model(task),
            reasoning_effort: self.api.reasoning_effort.clone(),
        }
    }

    /// The effective connection info for `task`: resolves
    /// `task_preset_id(task)` -> preset -> provider, falling back to the
    /// legacy per-section fields (unchanged behavior) whenever the preset or
    /// its provider can't be found.
    pub fn resolve_llm(&self, task: LlmTask) -> ResolvedLlm {
        let preset_id = self.task_preset_id(task);
        if !preset_id.is_empty() {
            if let Some(preset) = self.preset(preset_id) {
                if let Some(provider) = self.provider(&preset.provider_id) {
                    let base_url = if !provider.base_url.is_empty() {
                        provider.base_url.clone()
                    } else {
                        self.api.base_url.clone()
                    };
                    let api_key = if !provider.api_key.is_empty() {
                        provider.api_key.clone()
                    } else {
                        self.api.api_key.clone()
                    };
                    let model = if !preset.model.is_empty() {
                        preset.model.clone()
                    } else {
                        self.legacy_model(task)
                    };
                    let reasoning_effort = if !preset.reasoning_effort.is_empty() {
                        preset.reasoning_effort.clone()
                    } else {
                        self.api.reasoning_effort.clone()
                    };
                    return ResolvedLlm {
                        base_url,
                        api_key,
                        model,
                        reasoning_effort,
                    };
                }
            }
        }
        self.legacy_llm(task)
    }
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
        config.migrate_llm_config();
        Ok(config)
    }

    /// One-time, idempotent, in-memory migration from the legacy `api.*` /
    /// per-section fields to `providers[]` / `presets[]` (see
    /// tc-docs/drafts/llm-settings-common-v1.md §2). Only runs when both
    /// `providers` and `presets` are empty, so it never overwrites a config
    /// that has already been saved in the new shape. Does not write back to
    /// disk — the new shape is only persisted once the settings UI saves.
    fn migrate_llm_config(&mut self) {
        if !self.providers.is_empty() || !self.presets.is_empty() {
            return;
        }

        let has_id = |providers: &[ProviderConfig], id: &str| providers.iter().any(|p| p.id == id);
        let has_preset_id = |presets: &[PresetConfig], id: &str| presets.iter().any(|p| p.id == id);

        // `default` provider/preset, built from `api.*`.
        if !has_id(&self.providers, "default") {
            self.providers.push(ProviderConfig {
                id: "default".to_string(),
                label: "既定".to_string(),
                base_url: self.api.base_url.clone(),
                api_key: self.api.api_key.clone(),
            });
        }
        if !has_preset_id(&self.presets, "default") {
            let label = if !self.api.model.is_empty() {
                self.api.model.clone()
            } else {
                "既定".to_string()
            };
            self.presets.push(PresetConfig {
                id: "default".to_string(),
                label,
                provider_id: "default".to_string(),
                model: self.api.model.clone(),
                reasoning_effort: self.api.reasoning_effort.clone(),
            });
        }
        self.default_preset_id = "default".to_string();

        // Embedding preset.
        if !self.api.embedding_model.is_empty() && !has_preset_id(&self.presets, "embedding") {
            self.presets.push(PresetConfig {
                id: "embedding".to_string(),
                label: self.api.embedding_model.clone(),
                provider_id: "default".to_string(),
                model: self.api.embedding_model.clone(),
                reasoning_effort: String::new(),
            });
            self.memory.embedding_preset_id = "embedding".to_string();
        }

        // tts / stt / vision: dedicated provider only if the section's own
        // `base_url` is set and differs from `api.base_url`; dedicated
        // preset only if the section's own `model` is set.
        let (tts_base_url, tts_api_key, tts_model) =
            (self.tts.base_url.clone(), self.tts.api_key.clone(), self.tts.model.clone());
        self.migrate_section_llm("tts", "TTS", &tts_base_url, &tts_api_key, &tts_model);
        let (stt_base_url, stt_api_key, stt_model) =
            (self.stt.base_url.clone(), self.stt.api_key.clone(), self.stt.model.clone());
        self.migrate_section_llm("stt", "STT", &stt_base_url, &stt_api_key, &stt_model);
        let (vision_base_url, vision_api_key, vision_model) = (
            self.vision.base_url.clone(),
            self.vision.api_key.clone(),
            self.vision.model.clone(),
        );
        self.migrate_section_llm("vision", "Vision", &vision_base_url, &vision_api_key, &vision_model);
        if !self.tts.model.is_empty() {
            self.tts.preset_id = "tts".to_string();
        }
        if !self.stt.model.is_empty() {
            self.stt.preset_id = "stt".to_string();
        }
        if !self.vision.model.is_empty() {
            self.vision.preset_id = "vision".to_string();
        }

        // translation preset.
        if !self.translation.model.is_empty() && !has_preset_id(&self.presets, "translation") {
            self.presets.push(PresetConfig {
                id: "translation".to_string(),
                label: "通訳".to_string(),
                provider_id: "default".to_string(),
                model: self.translation.model.clone(),
                reasoning_effort: String::new(),
            });
            self.translation.preset_id = "translation".to_string();
        }
    }

    /// Shared helper for the tts/stt/vision legs of [`Config::migrate_llm_config`]:
    /// builds an `id`-named provider (only if `base_url` is set and differs
    /// from `api.base_url`) and an `id`-named preset (only if `model` is
    /// set), without touching the section's own `preset_id` — the caller
    /// assigns that afterward, once for all three sections.
    fn migrate_section_llm(&mut self, id: &str, label: &str, base_url: &str, api_key: &str, model: &str) {
        let has_id = |providers: &[ProviderConfig], id: &str| providers.iter().any(|p| p.id == id);
        let has_preset_id = |presets: &[PresetConfig], id: &str| presets.iter().any(|p| p.id == id);

        let provider_id = if !base_url.is_empty() && base_url != self.api.base_url {
            if !has_id(&self.providers, id) {
                self.providers.push(ProviderConfig {
                    id: id.to_string(),
                    label: label.to_string(),
                    base_url: base_url.to_string(),
                    api_key: api_key.to_string(),
                });
            }
            id.to_string()
        } else {
            "default".to_string()
        };

        if !model.is_empty() && !has_preset_id(&self.presets, id) {
            self.presets.push(PresetConfig {
                id: id.to_string(),
                label: label.to_string(),
                provider_id,
                model: model.to_string(),
                reasoning_effort: String::new(),
            });
        }
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

/// Restore `"***"`-masked `providers[].api_key` entries in an incoming
/// (about-to-be-saved) config document, matching them to `current` by
/// provider `id` rather than array position — a provider added, removed, or
/// reordered in the same edit still round-trips every other provider's real
/// key. Mirrors the `"***"` sentinel and walk-and-restore approach of
/// `npc-server/src/rest.rs`'s `restore_masked_secrets` (which already
/// restores everything else generically); this is the one spot that needs
/// id-based matching instead of positional matching, because the web UI's
/// provider list can be reordered independently of `current`.
pub fn unmask_provider_keys(incoming: &mut Value, current: &Config) {
    let Some(providers) = incoming.get_mut("providers").and_then(Value::as_array_mut) else {
        return;
    };
    for provider in providers.iter_mut() {
        let Some(obj) = provider.as_object_mut() else {
            continue;
        };
        let is_masked = matches!(obj.get("api_key"), Some(Value::String(s)) if s == "***");
        if !is_masked {
            continue;
        }
        let id = obj.get("id").and_then(Value::as_str).unwrap_or("").to_string();
        if let Some(real) = current.providers.iter().find(|p| p.id == id) {
            obj.insert("api_key".to_string(), Value::String(real.api_key.clone()));
        }
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
    fn affect_config_defaults_to_enabled_for_an_old_config() {
        // A config.json written before `talk.affect` existed.
        let config: Config = serde_json::from_str(r#"{"talk": {"enabled": true}}"#).unwrap();
        assert!(config.talk.affect.enabled);
        assert!(config.talk.affect.forced_closure);
        assert!(Config::default().talk.affect.enabled);
        assert!(Config::default().talk.affect.forced_closure);
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

    #[test]
    fn redacts_provider_api_keys_too() {
        let mut config = Config::default();
        config.providers.push(ProviderConfig {
            id: "p1".to_string(),
            label: "P1".to_string(),
            base_url: "http://x".to_string(),
            api_key: "sk-provider-secret".to_string(),
        });
        let redacted = config.redacted_json().unwrap();
        assert_eq!(redacted["providers"][0]["api_key"], "***");
    }

    #[test]
    fn resolve_llm_falls_back_to_legacy_fields_when_no_providers_or_presets() {
        let mut config = Config::default();
        config.api.base_url = "http://api".to_string();
        config.api.api_key = "api-key".to_string();
        config.api.model = "api-model".to_string();
        config.api.embedding_model = "embed-model".to_string();
        config.api.reasoning_effort = "medium".to_string();

        let talk = config.resolve_llm(LlmTask::Talk);
        assert_eq!(talk.base_url, "http://api");
        assert_eq!(talk.api_key, "api-key");
        assert_eq!(talk.model, "api-model");
        assert_eq!(talk.reasoning_effort, "medium");

        let embedding = config.resolve_llm(LlmTask::Embedding);
        assert_eq!(embedding.model, "embed-model");

        // vision: empty section fields fall back to api.*
        let vision = config.resolve_llm(LlmTask::Vision);
        assert_eq!(vision.base_url, "http://api");
        assert_eq!(vision.model, "api-model");

        // vision: section fields override api.* when set
        config.vision.base_url = "http://vision".to_string();
        config.vision.api_key = "vision-key".to_string();
        config.vision.model = "vision-model".to_string();
        let vision = config.resolve_llm(LlmTask::Vision);
        assert_eq!(vision.base_url, "http://vision");
        assert_eq!(vision.api_key, "vision-key");
        assert_eq!(vision.model, "vision-model");

        // tts: model falls back to api.model when tts.model is empty
        // (matches npc-speech's `synthesize_tts`, not just base_url/api_key).
        let tts = config.resolve_llm(LlmTask::Tts);
        assert_eq!(tts.model, "api-model");
        config.tts.model = "tts-model".to_string();
        let tts = config.resolve_llm(LlmTask::Tts);
        assert_eq!(tts.model, "tts-model");

        // stt: model has its own non-empty default and is never overridden
        // by api.model (matches npc-speech's `transcribe_and_publish`).
        let stt = config.resolve_llm(LlmTask::Stt);
        assert_eq!(stt.model, "whisper-1");
    }

    #[test]
    fn resolve_llm_uses_preset_and_provider_when_assigned() {
        let mut config = Config::default();
        config.api.base_url = "http://legacy".to_string();
        config.providers.push(ProviderConfig {
            id: "p1".to_string(),
            label: "P1".to_string(),
            base_url: "http://provider".to_string(),
            api_key: "provider-key".to_string(),
        });
        config.presets.push(PresetConfig {
            id: "preset1".to_string(),
            label: "Preset 1".to_string(),
            provider_id: "p1".to_string(),
            model: "preset-model".to_string(),
            reasoning_effort: "high".to_string(),
        });
        config.default_preset_id = "preset1".to_string();

        // Talk has no preset_id of its own -> falls back to default_preset_id.
        let talk = config.resolve_llm(LlmTask::Talk);
        assert_eq!(talk.base_url, "http://provider");
        assert_eq!(talk.api_key, "provider-key");
        assert_eq!(talk.model, "preset-model");
        assert_eq!(talk.reasoning_effort, "high");

        // A task-specific preset_id pointing at a preset with a dangling
        // provider_id falls back to the legacy path entirely.
        config.action.preset_id = "preset1".to_string();
        config.presets[0].provider_id = "missing".to_string();
        config.api.model = "legacy-action-model".to_string();
        let action = config.resolve_llm(LlmTask::Action);
        assert_eq!(action.base_url, "http://legacy");
        assert_eq!(action.model, "legacy-action-model");
    }

    #[test]
    fn task_preset_id_falls_back_to_default_preset_id() {
        let mut config = Config::default();
        config.default_preset_id = "d1".to_string();
        assert_eq!(config.task_preset_id(LlmTask::Talk), "d1");
        config.talk.preset_id = "custom".to_string();
        assert_eq!(config.task_preset_id(LlmTask::Talk), "custom");
        assert_eq!(config.task_preset_id(LlmTask::Embedding), "d1");
        config.memory.embedding_preset_id = "emb".to_string();
        assert_eq!(config.task_preset_id(LlmTask::Embedding), "emb");
    }

    #[test]
    fn migrate_llm_config_builds_default_provider_and_preset_from_api() {
        let mut config = Config::default();
        config.api.base_url = "http://api".to_string();
        config.api.api_key = "api-key".to_string();
        config.api.model = "api-model".to_string();
        config.api.embedding_model = "embed-model".to_string();
        config.api.reasoning_effort = "medium".to_string();

        config.migrate_llm_config();

        assert_eq!(config.default_preset_id, "default");
        let provider = config.provider("default").expect("default provider");
        assert_eq!(provider.base_url, "http://api");
        assert_eq!(provider.api_key, "api-key");
        let preset = config.preset("default").expect("default preset");
        assert_eq!(preset.provider_id, "default");
        assert_eq!(preset.model, "api-model");
        assert_eq!(preset.reasoning_effort, "medium");

        let embedding = config.preset("embedding").expect("embedding preset");
        assert_eq!(embedding.model, "embed-model");
        assert_eq!(config.memory.embedding_preset_id, "embedding");

        // Idempotent: running it again on an already-migrated config is a no-op.
        let providers_before = config.providers.len();
        let presets_before = config.presets.len();
        config.migrate_llm_config();
        assert_eq!(config.providers.len(), providers_before);
        assert_eq!(config.presets.len(), presets_before);
    }

    #[test]
    fn migrate_llm_config_skips_when_providers_or_presets_already_exist() {
        let mut config = Config::default();
        config.api.model = "should-not-be-used".to_string();
        config.providers.push(ProviderConfig {
            id: "existing".to_string(),
            label: "Existing".to_string(),
            base_url: "http://existing".to_string(),
            api_key: String::new(),
        });

        config.migrate_llm_config();

        assert_eq!(config.providers.len(), 1);
        assert!(config.presets.is_empty());
        assert_eq!(config.default_preset_id, "");
    }

    #[test]
    fn migrate_llm_config_gives_tts_stt_vision_dedicated_providers_only_when_distinct() {
        let mut config = Config::default();
        config.api.base_url = "http://api".to_string();
        config.tts.base_url = "http://api".to_string(); // same as api.base_url -> no dedicated provider
        config.tts.model = "tts-model".to_string();
        config.vision.base_url = "http://vision".to_string(); // distinct -> dedicated provider
        config.vision.api_key = "vision-key".to_string();
        config.vision.model = "vision-model".to_string();
        // stt: model left at its non-empty default ("whisper-1") but base_url
        // empty -> provider_id falls back to "default".

        config.migrate_llm_config();

        let tts_preset = config.preset("tts").expect("tts preset");
        assert_eq!(tts_preset.provider_id, "default");
        assert_eq!(config.tts.preset_id, "tts");

        let vision_preset = config.preset("vision").expect("vision preset");
        assert_eq!(vision_preset.provider_id, "vision");
        assert_eq!(config.vision.preset_id, "vision");
        let vision_provider = config.provider("vision").expect("vision provider");
        assert_eq!(vision_provider.base_url, "http://vision");
        assert_eq!(vision_provider.api_key, "vision-key");

        let stt_preset = config.preset("stt").expect("stt preset (non-empty default model)");
        assert_eq!(stt_preset.provider_id, "default");
        assert_eq!(config.stt.preset_id, "stt");
    }

    #[test]
    fn migrate_llm_config_leaves_translation_preset_id_empty_when_model_unset() {
        let mut config = Config::default();
        config.migrate_llm_config();
        assert_eq!(config.translation.preset_id, "");
        assert!(config.preset("translation").is_none());

        let mut config = Config::default();
        config.translation.model = "translate-model".to_string();
        config.migrate_llm_config();
        assert_eq!(config.translation.preset_id, "translation");
        assert_eq!(config.preset("translation").unwrap().model, "translate-model");
    }

    #[test]
    fn unmask_provider_keys_restores_by_id_not_position() {
        let mut current = Config::default();
        current.providers.push(ProviderConfig {
            id: "p1".to_string(),
            label: "P1".to_string(),
            base_url: "http://p1".to_string(),
            api_key: "real-p1-key".to_string(),
        });
        current.providers.push(ProviderConfig {
            id: "p2".to_string(),
            label: "P2".to_string(),
            base_url: "http://p2".to_string(),
            api_key: "real-p2-key".to_string(),
        });

        // Incoming reorders p2 before p1 and masks both keys; positional
        // matching would swap the restored secrets, id-based matching must not.
        let mut incoming = serde_json::json!({
            "providers": [
                { "id": "p2", "label": "P2", "base_url": "http://p2", "api_key": "***" },
                { "id": "p1", "label": "P1", "base_url": "http://p1", "api_key": "***" }
            ]
        });

        unmask_provider_keys(&mut incoming, &current);

        assert_eq!(incoming["providers"][0]["api_key"], "real-p2-key");
        assert_eq!(incoming["providers"][1]["api_key"], "real-p1-key");
    }

    #[test]
    fn unmask_provider_keys_leaves_unmasked_keys_untouched() {
        let current = Config::default();
        let mut incoming = serde_json::json!({
            "providers": [
                { "id": "new", "label": "New", "base_url": "http://new", "api_key": "freshly-typed-key" }
            ]
        });
        unmask_provider_keys(&mut incoming, &current);
        assert_eq!(incoming["providers"][0]["api_key"], "freshly-typed-key");
    }
}
