package config

import "strings"

// TranslationMode describes whether translations accompany or replace NPC replies.
type TranslationMode string

const (
	TranslationModeOff       TranslationMode = "off"
	TranslationModeInterpret TranslationMode = "interpret"
	TranslationModeAssist    TranslationMode = "assist"
)

func (m TranslationMode) IsOff() bool { return m == TranslationModeOff }

func (m TranslationMode) String() string { return string(m) }

// TranslationScope selects which half of the conversation is subtitled.
type TranslationScope string

const (
	TranslationScopeInput  TranslationScope = "input"
	TranslationScopeOutput TranslationScope = "output"
	TranslationScopeBoth   TranslationScope = "both"
)

func (s TranslationScope) TranslatesInput() bool {
	return s == TranslationScopeInput || s == TranslationScopeBoth
}

func (s TranslationScope) TranslatesOutput() bool {
	return s == TranslationScopeOutput || s == TranslationScopeBoth
}

func (s TranslationScope) String() string { return string(s) }

// Mode recognizes the configured mode, with off as the fallback.
func (c *TranslationConfig) Mode() TranslationMode {
	switch asciiLower(strings.TrimSpace(c.ModeValue)) {
	case "interpret":
		return TranslationModeInterpret
	case "assist":
		return TranslationModeAssist
	default:
		return TranslationModeOff
	}
}

// Scope recognizes the configured scope, with both as the fallback.
func (c *TranslationConfig) Scope() TranslationScope {
	switch asciiLower(strings.TrimSpace(c.ScopeValue)) {
	case "input":
		return TranslationScopeInput
	case "output":
		return TranslationScopeOutput
	default:
		return TranslationScopeBoth
	}
}

// SuppressesChat reports whether this session uses the NPC as an interpreter.
// Scope only selects subtitles and has no effect on replies.
func (c *TranslationConfig) SuppressesChat() bool {
	return c.Mode() == TranslationModeInterpret
}

// TargetLanguages returns configured targets in order, trimmed and deduplicated.
func (c *TranslationConfig) TargetLanguages() []string {
	languages := []string{}
	first := strings.TrimSpace(c.TargetLanguage)
	second := strings.TrimSpace(c.TargetLanguage2)
	if first != "" {
		languages = append(languages, first)
	}
	if second != "" && second != first {
		languages = append(languages, second)
	}
	return languages
}

// TranslationConfig configures simultaneous interpretation.
type TranslationConfig struct {
	ModeValue string `json:"mode"`
	// Selects the halves to subtitle independently of whether the NPC replies.
	ScopeValue      string `json:"scope"`
	SourceLanguage  string `json:"source_language"`
	TargetLanguage  string `json:"target_language"`
	TargetLanguage2 string `json:"target_language_2"`
	ContextSize     uint32 `json:"context_size"`
	// Translates back into SourceLanguage when the detected language differs.
	AutoReverse     bool      `json:"auto_reverse"`
	Chatbox         bool      `json:"chatbox"`
	Model           string    `json:"model"`
	ModelRef        *ModelRef `json:"model_ref,omitempty"`
	ReasoningEffort string    `json:"reasoning_effort"`
	PresetID        string    `json:"-" input:"preset_id"`
}

// DefaultTranslationConfig returns the Rust defaults for TranslationConfig.
func DefaultTranslationConfig() TranslationConfig {
	return TranslationConfig{
		ModeValue:      "off",
		ScopeValue:     "both",
		SourceLanguage: "日本語",
		TargetLanguage: "英語",
		ContextSize:    3,
		AutoReverse:    true,
	}
}

func (c *TranslationConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultTranslationConfig())
}
