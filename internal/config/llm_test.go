package config

import (
	"path/filepath"
	"strings"
	"testing"
)

func modelRef(provider, model string) *ModelRef {
	return &ModelRef{ProviderID: provider, Model: model}
}

func modelConfig() Config {
	cfg := DefaultConfig()
	cfg.addLegacyProvider("http", "", "http://endpoint/v1", "")
	cfg.addLegacyProvider("room", "", "mist-network://team", "")
	cfg.DefaultRef = modelRef("http", "default-model")
	return cfg
}

func resolve(t *testing.T, cfg *Config, task LlmTask) ResolvedLlm {
	t.Helper()
	result, err := cfg.Resolve(task)
	if err != nil {
		t.Fatal(err)
	}
	return result
}

func resolveError(t *testing.T, cfg *Config, task LlmTask, message string) {
	t.Helper()
	_, err := cfg.Resolve(task)
	if err == nil || !strings.Contains(err.Error(), message) {
		t.Errorf("Resolve(%s) error = %v, want error containing %q", task, err, message)
	}
}

func TestDisabledProviderFallsBackWithoutRewritingAssignment(t *testing.T) {
	cfg := modelConfig()
	cfg.Translation.ModelRef = modelRef("room", "room-model")
	cfg.Translation.ReasoningEffort = "high"
	cfg.Providers[1].Enabled = false
	stored := encode(t, cfg)
	result := resolve(t, &cfg, TaskTranslation)
	equal(t, result.ProviderID, "http")
	equal(t, result.Model, "default-model")
	equal(t, result.ReasoningEffort, "high")
	equal(t, encode(t, cfg), stored)
	cfg.Providers[0].Enabled = false
	resolveError(t, &cfg, TaskTranslation, "default provider is disabled")
}

func TestMissingOrBlankRefsErrorAndNeverBorrowLegacyCredentials(t *testing.T) {
	cfg := modelConfig()
	cfg.API.APIKey = "legacy-secret"
	equal(t, resolve(t, &cfg, TaskTalk).APIKey, "")
	cfg.Talk.ModelRef = modelRef("missing", "m")
	resolveError(t, &cfg, TaskTalk, `provider "missing" is missing`)
	cfg.Talk.ModelRef = modelRef("http", " ")
	resolveError(t, &cfg, TaskTalk, "model is empty")
	cfg.Talk.ModelRef, cfg.DefaultRef = nil, nil
	resolveError(t, &cfg, TaskTalk, "no model reference configured")
	defaults := DefaultConfig()
	resolveError(t, &defaults, TaskTalk, "no model reference configured")
}

func TestRoomRefsUseMistlBridgeAndRawModelForEveryUse(t *testing.T) {
	cfg := modelConfig()
	cfg.DefaultRef = modelRef("room", "raw-model")
	cfg.Mist.AIBaseURL = "http://127.0.0.1:16478/v1"
	cfg.Providers[1].APIKey = "must-not-leak"
	for _, task := range llmTasks {
		t.Run(string(task), func(t *testing.T) {
			result := resolve(t, &cfg, task)
			equal(t, result.ProviderID, "room")
			equal(t, result.BaseURL, cfg.Mist.AIBaseURL+"/rooms/team")
			equal(t, result.Model, "raw-model")
			equal(t, result.APIKey, "")
		})
	}
	cfg.TTS.ModelRef = modelRef("room", "network-auto")
	equal(t, resolve(t, &cfg, TaskTTS).Model, "network-auto")
}

func TestMigrationPreservesAllUsesEffortCacheAndNewAssignments(t *testing.T) {
	cfg := decode[Config](t, `{
		"providers":[
			{"id":"http","base_url":"http://endpoint","models":["cached"]},
			{"id":"room","base_url":"mist-network://team","enabled":false}
		],
		"presets":[
			{"id":"old","provider_id":"http","model":"manual-model","reasoning_effort":"high","temperature":0.7},
			{"id":"remote","provider_id":"room","model":"raw-model"}
		],
		"default_preset_id":"old",
		"talk":{"preset_id":"old","reasoning_effort":"low"},
		"memory":{"preset_id":"old","embedding_preset_id":"old"},
		"vision":{"preset_id":"old","model_ref":{"provider_id":"room","model":"selected"}},
		"action":{"preset_id":"old"},
		"translation":{"preset_id":"old"},
		"tts":{"preset_id":"remote"},
		"stt":{"preset_id":"old"}
	}`)
	equal(t, cfg.MigrateLLMConfig(), true)
	deepEqual(t, cfg.DefaultRef, modelRef("http", "manual-model"))
	deepEqual(t, cfg.Memory.EmbeddingRef, cfg.DefaultRef)
	equal(t, cfg.Talk.ReasoningEffort, "low")
	equal(t, cfg.Memory.ReasoningEffort, "high")
	equal(t, cfg.Translation.ReasoningEffort, "high")
	deepEqual(t, cfg.Vision.ModelRef, modelRef("room", "selected"))
	deepEqual(t, cfg.TTS.ModelRef, modelRef("room", "raw-model"))
	deepEqual(t, cfg.Providers[0].Models, []string{"cached", "manual-model"})
	equal(t, len(cfg.Providers[1].Models), 0)
	equal(t, cfg.Providers[1].Enabled, false)
	serialized := encode(t, cfg)
	for _, key := range []string{"preset", "temperature"} {
		if strings.Contains(serialized, key) {
			t.Errorf("serialization retained legacy key %q", key)
		}
	}
	equal(t, cfg.MigrateLLMConfig(), false)
	roundTrip := decode[Config](t, serialized)
	deepEqual(t, roundTrip.Providers[0].Models, cfg.Providers[0].Models)
	deepEqual(t, roundTrip.DefaultRef, cfg.DefaultRef)
}

func TestLegacyDirectFieldsMigrateWithDistinctCredentials(t *testing.T) {
	cfg := DefaultConfig()
	cfg.API.Model = "chat-model"
	cfg.API.APIKey = "chat-key"
	cfg.API.EmbeddingModel = "embedding-model"
	cfg.TTS.Model = "speech-model"
	cfg.TTS.APIKey = "voice-key"
	cfg.Vision.BaseURL = "http://vision/v1"
	cfg.Vision.Model = "vision-model"
	equal(t, cfg.MigrateLLMConfig(), true)
	equal(t, resolve(t, &cfg, TaskTalk).Model, "chat-model")
	equal(t, resolve(t, &cfg, TaskEmbedding).Model, "embedding-model")
	tts := resolve(t, &cfg, TaskTTS)
	equal(t, tts.APIKey, "voice-key")
	equal(t, tts.BaseURL, cfg.API.BaseURL)
	equal(t, resolve(t, &cfg, TaskVision).BaseURL, "http://vision/v1")
	equal(t, cfg.MigrateLLMConfig(), false)
}

func TestUnresolvedLegacyPresetAndNewDefaultSurviveMigration(t *testing.T) {
	cfg := modelConfig()
	cfg.DefaultPresetID = "unknown-default"
	cfg.Talk.PresetID = "unknown-task"
	equal(t, cfg.MigrateLLMConfig(), true)
	deepEqual(t, cfg.DefaultRef, modelRef("http", "default-model"))
	deepEqual(t, cfg.Talk.ModelRef, modelRef("", "unknown-task"))
	resolveError(t, &cfg, TaskTalk, `provider "" is missing`)
	equal(t, cfg.MigrateLLMConfig(), false)
}

func TestLoadPersistsMigrationOnceAndExampleUsesCurrentSchema(t *testing.T) {
	example := decode[Config](t, exampleJSON(t))
	path := filepath.Join(isolateLoadEnv(t), "npc-refs.json")
	writeFixture(t, path, `{
		"providers":[{"id":"http","base_url":"http://endpoint"}],
		"presets":[{"id":"old","provider_id":"http","model":"raw"}],
		"default_preset_id":"old"
	}`)
	cfg, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	equal(t, resolve(t, cfg, TaskTalk).Model, "raw")
	saved := readFixture(t, path)
	if strings.Contains(saved, "preset") {
		t.Error("saved config retained legacy presets")
	}
	if _, err := Load(path); err != nil {
		t.Fatal(err)
	}
	equal(t, readFixture(t, path), saved)
	equal(t, example.MigrateLLMConfig(), false)
	for _, task := range llmTasks {
		if resolve(t, &example, task).Model == "" {
			t.Errorf("example has empty model for %s", task)
		}
	}
}

func TestResolveRoomEncodingAndErrors(t *testing.T) {
	cfg := modelConfig()
	cfg.Mist.AIBaseURL = "http://bridge/v1///"
	cfg.DefaultRef = modelRef("room", " model ")
	cfg.Providers[1].BaseURL = "mist-network://room /%+$日本"
	result := resolve(t, &cfg, TaskTalk)
	equal(t, result.BaseURL, "http://bridge/v1/rooms/room%20%2F%25%2B%24%E6%97%A5%E6%9C%AC")
	equal(t, result.Model, " model ")
	cfg.Mist.AIBaseURL = " "
	resolveError(t, &cfg, TaskTalk, "mistl AI serve URL is empty")
	cfg.Providers[1].BaseURL = "mist-network:// "
	resolveError(t, &cfg, TaskTalk, "room ID is empty")
	cfg.Providers[1].BaseURL = " "
	resolveError(t, &cfg, TaskTalk, "provider URL is empty")

	cfg.Providers[1].Enabled = false
	cfg.Memory.EmbeddingRef, cfg.DefaultRef = modelRef("room", "m"), nil
	resolveError(t, &cfg, TaskEmbedding, "provider is disabled and no default is configured")
	cfg.DefaultRef = modelRef("missing", "m")
	resolveError(t, &cfg, TaskEmbedding, "default provider is missing")
}
