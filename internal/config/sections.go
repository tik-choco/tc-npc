package config

import "encoding/json"

const defaultOSCAddress = "127.0.0.1:9000"

// APIConfig contains the legacy chat and embedding connection and default effort.
type APIConfig struct {
	BaseURL string `json:"base_url"`
	// Secret. Use RedactedJSON when displaying the config.
	APIKey         string `json:"api_key"`
	Model          string `json:"model"`
	EmbeddingModel string `json:"embedding_model"`
	// Sent verbatim on chat requests, including the explicit value "none".
	ReasoningEffort string `json:"reasoning_effort"`
}

// DefaultAPIConfig returns the Rust defaults for APIConfig.
func DefaultAPIConfig() APIConfig {
	return APIConfig{
		BaseURL:         "http://localhost:11434/v1",
		ReasoningEffort: "none",
	}
}

func (c *APIConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultAPIConfig())
}

// TTSConfig configures speech synthesis.
type TTSConfig struct {
	Enabled  bool      `json:"enabled"`
	BaseURL  string    `json:"base_url"`
	APIKey   string    `json:"api_key"`
	Model    string    `json:"model"`
	Voice    string    `json:"voice"`
	Speed    float32   `json:"speed"`
	MaxLen   uint32    `json:"max_len"`
	ModelRef *ModelRef `json:"model_ref,omitempty"`
	PresetID string    `json:"-" input:"preset_id"`
}

// DefaultTTSConfig returns the Rust defaults for TTSConfig.
func DefaultTTSConfig() TTSConfig {
	return TTSConfig{
		Voice:  "alloy",
		Speed:  1.0,
		MaxLen: 200,
	}
}

func (c *TTSConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultTTSConfig())
}

// STTConfig configures speech recognition.
type STTConfig struct {
	Enabled         bool    `json:"enabled"`
	BaseURL         string  `json:"base_url"`
	APIKey          string  `json:"api_key"`
	Model           string  `json:"model"`
	SilenceDuration float32 `json:"silence_duration"`
	InputThreshold  float32 `json:"input_threshold"`
	// Keeps the mic active during TTS so a user can interrupt playback.
	BargeIn bool `json:"barge_in"`
	// During TTS, mic level must cross InputThreshold times this factor.
	// The higher threshold keeps a reply from interrupting itself.
	BargeInFactor float32   `json:"barge_in_factor"`
	ModelRef      *ModelRef `json:"model_ref,omitempty"`
	PresetID      string    `json:"-" input:"preset_id"`
}

// DefaultSTTConfig returns the Rust defaults for STTConfig.
func DefaultSTTConfig() STTConfig {
	return STTConfig{
		SilenceDuration: 1.5,
		InputThreshold:  0.01,
		BargeIn:         true,
		BargeInFactor:   2.5,
	}
}

func (c *STTConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultSTTConfig())
}

// PromptEntry is a named prompt template. Placeholders such as {{persona}} and
// {{short_term_memory}} are expanded by the conversation module.
type PromptEntry struct {
	Name    string `json:"name" required:"true"`
	Content string `json:"content" required:"true"`
}

func (c *PromptEntry) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, PromptEntry{})
}

// TalkConfig configures conversation and spoken replies.
type TalkConfig struct {
	Enabled          bool          `json:"enabled"`
	HistorySize      uint32        `json:"history_size"`
	Prompts          []PromptEntry `json:"prompts"`
	FilterPromptName string        `json:"filter_prompt_name"`
	// Injects the built-in spoken-reply rules even when Prompts is empty.
	StyleRules bool `json:"style_rules"`
	// Lets the NPC leave an utterance unanswered and end a conversation naturally.
	AllowSilence    bool         `json:"allow_silence"`
	Affect          AffectConfig `json:"affect"`
	ModelRef        *ModelRef    `json:"model_ref,omitempty"`
	ReasoningEffort string       `json:"reasoning_effort"`
	PresetID        string       `json:"-" input:"preset_id"`
}

// DefaultTalkConfig returns the Rust defaults for TalkConfig.
func DefaultTalkConfig() TalkConfig {
	return TalkConfig{
		Enabled:      true,
		HistorySize:  20,
		Prompts:      []PromptEntry{},
		StyleRules:   true,
		AllowSilence: true,
		Affect:       DefaultAffectConfig(),
	}
}

func (c *TalkConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultTalkConfig())
}

func (c TalkConfig) MarshalJSON() ([]byte, error) {
	type wire TalkConfig
	c.Prompts = nonNilSlice(c.Prompts)
	return json.Marshal(wire(c))
}

// AffectConfig controls the conversation drive model and forced closures.
type AffectConfig struct {
	Enabled       bool `json:"enabled"`
	ForcedClosure bool `json:"forced_closure"`
	// After this silence, reset the drives and closing state while keeping familiarity.
	// Zero disables the check.
	AbsenceTimeoutSecs uint64 `json:"absence_timeout_secs"`
}

// DefaultAffectConfig returns the Rust defaults for AffectConfig.
func DefaultAffectConfig() AffectConfig {
	return AffectConfig{
		Enabled:            true,
		ForcedClosure:      true,
		AbsenceTimeoutSecs: 300,
	}
}

func (c *AffectConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultAffectConfig())
}

// MemoryConfig controls conversation memory and the independent embedding task.
type MemoryConfig struct {
	Enabled            bool               `json:"enabled"`
	IdleTimeoutMinutes uint32             `json:"idle_timeout_minutes"`
	TopK               uint32             `json:"top_k"`
	Threshold          float32            `json:"threshold"`
	ChunkSize          uint32             `json:"chunk_size"`
	ChunkOverlap       uint32             `json:"chunk_overlap"`
	SummarizePrompt    string             `json:"summarize_prompt"`
	ConsolidatePrompt  string             `json:"consolidate_prompt"`
	People             PersonMemoryConfig `json:"people"`
	ModelRef           *ModelRef          `json:"model_ref,omitempty"`
	ReasoningEffort    string             `json:"reasoning_effort"`
	PresetID           string             `json:"-" input:"preset_id"`
	// Selects the embedding model independently of the memory chat model.
	EmbeddingRef      *ModelRef `json:"embedding_ref,omitempty"`
	EmbeddingPresetID string    `json:"-" input:"embedding_preset_id"`
}

// DefaultMemoryConfig returns the Rust defaults for MemoryConfig.
func DefaultMemoryConfig() MemoryConfig {
	return MemoryConfig{
		Enabled:            true,
		IdleTimeoutMinutes: 5,
		TopK:               5,
		Threshold:          0.1,
		ChunkSize:          512,
		ChunkOverlap:       64,
		SummarizePrompt:    "以下の会話を短く要約してください。",
		ConsolidatePrompt:  "以下の記憶をまとめ、重複を除いて簡潔に整理してください。",
		People:             DefaultPersonMemoryConfig(),
	}
}

func (c *MemoryConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultMemoryConfig())
}

// PersonMemoryConfig controls facts extracted about conversation partners.
type PersonMemoryConfig struct {
	Enabled       bool   `json:"enabled"`
	ExtractPrompt string `json:"extract_prompt"`
	MaxFacts      uint32 `json:"max_facts"`
	// Includes people detected by the vision module in person memory.
	LinkVision bool `json:"link_vision"`
}

// DefaultPersonMemoryConfig returns the Rust defaults for PersonMemoryConfig.
func DefaultPersonMemoryConfig() PersonMemoryConfig {
	return PersonMemoryConfig{
		Enabled:       true,
		ExtractPrompt: "以下の会話から、登場した人物について新たに分かった事実だけを抽出してください。JSON配列で、各要素は {\"name\": \"人物名\", \"facts\": [\"事実1\", \"事実2\"]} の形式にしてください。人物が特定できない、または新しい情報がない場合は空配列 [] を返してください。説明や前置きは不要です。",
		MaxFacts:      20,
		LinkVision:    true,
	}
}

func (c *PersonMemoryConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultPersonMemoryConfig())
}

// VisionConfig configures screen capture and image understanding.
type VisionConfig struct {
	Enabled           bool      `json:"enabled"`
	BaseURL           string    `json:"base_url"`
	APIKey            string    `json:"api_key"`
	Model             string    `json:"model"`
	IntervalSeconds   float32   `json:"interval_seconds"`
	CaptureMode       string    `json:"capture_mode"`
	TargetWindowTitle string    `json:"target_window_title"`
	SystemPrompt      string    `json:"system_prompt"`
	DebugSave         bool      `json:"debug_save"`
	PersonDetection   bool      `json:"person_detection"`
	ModelRef          *ModelRef `json:"model_ref,omitempty"`
	ReasoningEffort   string    `json:"reasoning_effort"`
	PresetID          string    `json:"-" input:"preset_id"`
}

// DefaultVisionConfig returns the Rust defaults for VisionConfig.
func DefaultVisionConfig() VisionConfig {
	return VisionConfig{
		IntervalSeconds: 5.0,
		CaptureMode:     "entire",
		SystemPrompt:    "画面のスクリーンショットを簡潔に日本語で説明してください。",
	}
}

func (c *VisionConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultVisionConfig())
}

// SpeechConfig configures local audio devices and sample rates.
type SpeechConfig struct {
	InputDevice      string `json:"input_device"`
	OutputDevice     string `json:"output_device"`
	InputSampleRate  uint32 `json:"input_sample_rate"`
	OutputSampleRate uint32 `json:"output_sample_rate"`
}

// DefaultSpeechConfig returns the Rust defaults for SpeechConfig.
func DefaultSpeechConfig() SpeechConfig {
	return SpeechConfig{
		InputSampleRate:  16000,
		OutputSampleRate: 44100,
	}
}

func (c *SpeechConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultSpeechConfig())
}

type LocationConfig struct {
	Name    string  `json:"name" required:"true"`
	X       float64 `json:"x"`
	Y       float64 `json:"y"`
	Heading float64 `json:"heading"`
}

func (c *LocationConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, LocationConfig{})
}

type WaypointConfig struct {
	Location string  `json:"location" required:"true"`
	Seconds  float64 `json:"seconds"`
	Wait     bool    `json:"wait"`
}

func (c *WaypointConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, WaypointConfig{})
}

type RouteConfig struct {
	Name      string           `json:"name" required:"true"`
	Loop      bool             `json:"loop"`
	Waypoints []WaypointConfig `json:"waypoints"`
}

func defaultRouteConfig() RouteConfig {
	return RouteConfig{
		Waypoints: []WaypointConfig{},
	}
}

func (c *RouteConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, defaultRouteConfig())
}

func (c RouteConfig) MarshalJSON() ([]byte, error) {
	type wire RouteConfig
	c.Waypoints = nonNilSlice(c.Waypoints)
	return json.Marshal(wire(c))
}

// ActionConfig controls OSC movement and named locations and routes.
type ActionConfig struct {
	Enabled         bool             `json:"enabled"`
	OSCAddress      string           `json:"osc_address"`
	Locations       []LocationConfig `json:"locations"`
	Routes          []RouteConfig    `json:"routes"`
	ModelRef        *ModelRef        `json:"model_ref,omitempty"`
	ReasoningEffort string           `json:"reasoning_effort"`
	PresetID        string           `json:"-" input:"preset_id"`
}

// DefaultActionConfig returns the Rust defaults for ActionConfig.
func DefaultActionConfig() ActionConfig {
	return ActionConfig{
		OSCAddress: defaultOSCAddress,
		Locations:  []LocationConfig{},
		Routes:     []RouteConfig{},
	}
}

func (c *ActionConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultActionConfig())
}

func (c ActionConfig) MarshalJSON() ([]byte, error) {
	type wire ActionConfig
	c.Locations = nonNilSlice(c.Locations)
	c.Routes = nonNilSlice(c.Routes)
	return json.Marshal(wire(c))
}

// VRCConfig controls VRChat chatbox output.
type VRCConfig struct {
	Chatbox    bool   `json:"chatbox"`
	OSCAddress string `json:"osc_address"`
}

// DefaultVRCConfig returns the Rust defaults for VRCConfig.
func DefaultVRCConfig() VRCConfig {
	return VRCConfig{
		OSCAddress: defaultOSCAddress,
	}
}

func (c *VRCConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultVRCConfig())
}

// ServerConfig configures the local web server.
type ServerConfig struct {
	Addr     string `json:"addr"`
	AutoOpen bool   `json:"auto_open"`
}

// DefaultServerConfig returns the Rust defaults for ServerConfig.
func DefaultServerConfig() ServerConfig {
	return ServerConfig{
		Addr:     "127.0.0.1:47950",
		AutoOpen: true,
	}
}

func (c *ServerConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultServerConfig())
}

// CharacterConfig selects the active character and fallback avatar.
type CharacterConfig struct {
	ActiveID string `json:"active_id"`
	// File in DataDir()/vrm used when the active character has no avatar.
	AvatarFile string `json:"avatar_file"`
}

func (c *CharacterConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, CharacterConfig{})
}

// MistConfig configures the embedded mist engine and mistl AI serve bridge.
type MistConfig struct {
	CLIPath  string  `json:"cli_path"`
	Instance *string `json:"instance,omitempty"`
	StateDir *string `json:"state_dir,omitempty"`
	// Local OpenAI-compatible bridge started with mistl ai serve.
	AIBaseURL    string           `json:"ai_base_url"`
	Enabled      bool             `json:"enabled"`
	SignalingURL string           `json:"signaling_url"`
	RoomID       string           `json:"room_id"`
	AvatarLink   AvatarLinkConfig `json:"avatar_link"`
}

// DefaultMistConfig returns the Rust defaults for MistConfig.
func DefaultMistConfig() MistConfig {
	return MistConfig{
		AIBaseURL:  "http://127.0.0.1:6478/v1",
		CLIPath:    "mistl",
		AvatarLink: DefaultAvatarLinkConfig(),
	}
}

func (c *MistConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultMistConfig())
}

// AvatarLinkConfig configures peer avatar discovery.
type AvatarLinkConfig struct {
	Enabled    bool     `json:"enabled"`
	RoomID     string   `json:"room_id"`
	Name       string   `json:"name"`
	AllowHosts []string `json:"allow_hosts"`
}

// DefaultAvatarLinkConfig returns the Rust defaults for AvatarLinkConfig.
func DefaultAvatarLinkConfig() AvatarLinkConfig {
	return AvatarLinkConfig{
		RoomID:     "tc-avatar-link-v1",
		AllowHosts: []string{},
	}
}

func (c *AvatarLinkConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultAvatarLinkConfig())
}

func (c AvatarLinkConfig) MarshalJSON() ([]byte, error) {
	type wire AvatarLinkConfig
	c.AllowHosts = nonNilSlice(c.AllowHosts)
	return json.Marshal(wire(c))
}
