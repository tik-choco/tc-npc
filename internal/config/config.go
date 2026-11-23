// Package config loads, migrates, and saves the tc-npc configuration document.
//
// Rust to Go API mapping:
//   - data_dir → DataDir; Config::{resolve_path, load, default} → ResolvePath,
//     Load, DefaultConfig. An empty path selects the default path.
//     Section defaults with nonzero fields or initialized slices use Default<Type>.
//   - Config::{provider, task_ref, resolve_llm, migrate_llm_config, redacted_json}
//     → (*Config).{Provider, TaskRef, Resolve, MigrateLLMConfig, RedactedJSON}.
//     Save exposes the pretty JSON file write used by load-time migration.
//   - ApiConfig, TtsConfig, SttConfig, VrcConfig → APIConfig, TTSConfig,
//     STTConfig, VRCConfig. ModelRef, LlmTask and ResolvedLlm keep their names.
//   - ProviderConfig::room → (*ProviderConfig).Room.
//   - ScheduledAction variants → ScheduledAction.Kind and ScheduledAction*
//     constants. Raw.msg_type → MessageType.
//   - AnnouncementConfig::effective_* → (*AnnouncementConfig).Effective*.
//   - TranslationConfig::{mode, scope} methods → Mode, Scope; the JSON-backed
//     fields are ModeValue and ScopeValue. TranslationMode/Scope::as_str and
//     Display → String. Other methods use PascalCase.
//   - language_instruction, with_language_instruction, unmask_provider_keys
//     → LanguageInstruction, WithLanguageInstruction, UnmaskProviderKeys.
package config

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"sync"
)

// Config is the top-level configuration document.
type Config struct {
	// Language selects the instruction to add to LLM prompts. See LanguageInstruction.
	Language        string               `json:"language"`
	API             APIConfig            `json:"api"`
	TTS             TTSConfig            `json:"tts"`
	STT             STTConfig            `json:"stt"`
	Talk            TalkConfig           `json:"talk"`
	Memory          MemoryConfig         `json:"memory"`
	Vision          VisionConfig         `json:"vision"`
	Speech          SpeechConfig         `json:"speech"`
	Action          ActionConfig         `json:"action"`
	VRC             VRCConfig            `json:"vrc"`
	Scheduler       SchedulerConfig      `json:"scheduler"`
	ScheduleProfile ScheduleProfileState `json:"schedule_profile"`
	Translation     TranslationConfig    `json:"translation"`
	Server          ServerConfig         `json:"server"`
	Character       CharacterConfig      `json:"character"`
	Mist            MistConfig           `json:"mist"`
	Providers       []ProviderConfig     `json:"providers"`
	DefaultRef      *ModelRef            `json:"default_ref,omitempty"`

	// Deserialize-only inputs consumed by MigrateLLMConfig.
	Presets         []PresetConfig `json:"-" input:"presets"`
	DefaultPresetID string         `json:"-" input:"default_preset_id"`
}

// DefaultConfig returns the defaults used when there is no config file.
// JSON decoding separately defaults a missing language to "auto".
func DefaultConfig() Config {
	return Config{
		API:         DefaultAPIConfig(),
		TTS:         DefaultTTSConfig(),
		STT:         DefaultSTTConfig(),
		Talk:        DefaultTalkConfig(),
		Memory:      DefaultMemoryConfig(),
		Vision:      DefaultVisionConfig(),
		Speech:      DefaultSpeechConfig(),
		Action:      DefaultActionConfig(),
		VRC:         DefaultVRCConfig(),
		Scheduler:   DefaultSchedulerConfig(),
		Translation: DefaultTranslationConfig(),
		Server:      DefaultServerConfig(),
		Mist:        DefaultMistConfig(),
		Providers:   []ProviderConfig{},
		Presets:     []PresetConfig{},
	}
}

func (c *Config) UnmarshalJSON(data []byte) error {
	defaults := DefaultConfig()
	defaults.Language = "auto"
	return decodeStruct(data, c, defaults)
}

func (c Config) MarshalJSON() ([]byte, error) {
	type wire Config
	c.Providers = nonNilSlice(c.Providers)
	return json.Marshal(wire(c))
}

// DataDirEnv overrides the data directory when set. Go-port addition for
// development and smoke tests, so a scratch instance never touches the real
// ~/.tc-npc (characters, memory, server-port.txt).
const DataDirEnv = "TC_NPC_DATA_DIR"

// DataDir returns the app data directory (~/.tc-npc, or $TC_NPC_DATA_DIR).
// Callers create it as needed.
func DataDir() string {
	if dir := os.Getenv(DataDirEnv); dir != "" {
		return dir
	}
	return filepath.Join(homeDir(), ".tc-npc")
}

// ResolvePath returns the explicit path as supplied, or DataDir()/config.json
// when explicit is empty.
func ResolvePath(explicit string) string {
	if explicit != "" {
		return explicit
	}
	return filepath.Join(DataDir(), "config.json")
}

var loadMu sync.Mutex

// Load loads .env, reads the config (or defaults), persists legacy migrations,
// and applies environment overrides. An empty path enables default-path migration.
func Load(path string) (*Config, error) {
	loadMu.Lock()
	defer loadMu.Unlock()

	loadDotEnv()
	resolved := ResolvePath(path)
	if path == "" {
		// Migration is best-effort so an unwritable default location still
		// allows startup with defaults.
		_ = migrateLegacyConfig(resolved, "config.json")
	}
	cfg := DefaultConfig()
	_, statErr := os.Stat(resolved)
	if statErr == nil {
		data, err := os.ReadFile(resolved)
		if err != nil {
			return nil, fmt.Errorf("failed to read config file %s: %w", resolved, err)
		}
		if err := json.Unmarshal(data, &cfg); err != nil {
			return nil, fmt.Errorf("failed to parse config file %s: %w", resolved, err)
		}
	}
	if cfg.MigrateLLMConfig() && statErr == nil {
		if _, err := os.Stat(resolved); err == nil {
			if err := cfg.Save(resolved); err != nil {
				return nil, err
			}
		}
	}
	cfg.applyEnvOverrides(os.LookupEnv)
	return &cfg, nil
}

// Save writes the current config, including credentials, as pretty JSON.
// An empty path selects DataDir()/config.json.
func (c *Config) Save(path string) error {
	path = ResolvePath(path)
	data, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to serialize config file %s: %w", path, err)
	}
	if err := writeConfigFile(path, data); err != nil {
		return fmt.Errorf("failed to write config file %s: %w", path, err)
	}
	return nil
}

func writeConfigFile(path string, data []byte) error {
	mode := os.FileMode(0o600)
	info, err := os.Lstat(path)
	if err == nil {
		if info.Mode()&os.ModeSymlink != 0 {
			// Preserve a config symlink by replacing the file it points to.
			path, err = filepath.EvalSymlinks(path)
			if err != nil {
				return err
			}
			info, err = os.Stat(path)
			if err != nil {
				return err
			}
		}
		if !info.Mode().IsRegular() {
			return fmt.Errorf("config path is not a regular file")
		}
		mode = info.Mode().Perm()
	} else if !errors.Is(err, os.ErrNotExist) {
		return err
	}
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	file, err := os.CreateTemp(dir, ".config-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(file.Name())
	_, writeErr := file.Write(data)
	if err := errors.Join(writeErr, file.Chmod(mode), file.Close()); err != nil {
		return err
	}
	return os.Rename(file.Name(), path)
}

const maskedAPIKey = "***"

// RedactedJSON returns a detached JSON object with nonempty API keys masked.
func (c *Config) RedactedJSON() (map[string]any, error) {
	data, err := json.Marshal(c)
	if err != nil {
		return nil, fmt.Errorf("serialize config for redaction: %w", err)
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	decoder.UseNumber()
	var doc map[string]any
	if err := decoder.Decode(&doc); err != nil {
		return nil, fmt.Errorf("decode config for redaction: %w", err)
	}
	widenJSONFloats(reflect.ValueOf(c), doc)
	redactAPIKeys(doc)
	return doc, nil
}

func redactAPIKeys(value any) {
	switch node := value.(type) {
	case map[string]any:
		for key, child := range node {
			if key == "api_key" {
				if secret, ok := child.(string); ok && secret != "" {
					node[key] = maskedAPIKey
				}
			} else {
				redactAPIKeys(child)
			}
		}
	case []any:
		for _, child := range node {
			redactAPIKeys(child)
		}
	}
}

// UnmaskProviderKeys restores masked providers' keys in an incoming JSON
// object. ID matching keeps credentials attached to providers during reordering.
func UnmaskProviderKeys(incoming map[string]any, current *Config) {
	providers, _ := incoming["providers"].([]any)
	for _, value := range providers {
		provider, _ := value.(map[string]any)
		if provider["api_key"] != maskedAPIKey {
			continue
		}
		id, _ := provider["id"].(string)
		if saved := current.Provider(id); saved != nil {
			provider["api_key"] = saved.APIKey
		}
	}
}

// LanguageInstruction returns the prompt instruction for ja, en, or zh.
// Auto, blank, and other values leave the wording up to the prompt.
func LanguageInstruction(language string) (string, bool) {
	switch asciiLower(strings.TrimSpace(language)) {
	case "ja":
		return "Always write your reply in Japanese (日本語).", true
	case "en":
		return "Always write your reply in English.", true
	case "zh":
		return "Always write your reply in Simplified Chinese (简体中文).", true
	default:
		return "", false
	}
}

// WithLanguageInstruction appends the instruction as its own paragraph.
func WithLanguageInstruction(prompt, language string) string {
	instruction, ok := LanguageInstruction(language)
	if !ok {
		return prompt
	}
	if strings.TrimSpace(prompt) == "" {
		return instruction
	}
	return prompt + "\n\n" + instruction
}

func asciiLower(s string) string {
	return strings.Map(func(r rune) rune {
		if r >= 'A' && r <= 'Z' {
			return r + ('a' - 'A')
		}
		return r
	}, s)
}
