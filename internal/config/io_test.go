package config

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestMigrateLegacyConfigCopiesWhenTargetMissing(t *testing.T) {
	dir := t.TempDir()
	legacy := filepath.Join(dir, "legacy", "config.json")
	target := filepath.Join(dir, "target", "config.json")
	content := `{"api":{"model":"legacy-model"}}`
	writeFixture(t, legacy, content)
	if err := migrateLegacyConfig(target, legacy); err != nil {
		t.Fatal(err)
	}
	equal(t, readFixture(t, target), content)
	equal(t, readFixture(t, legacy), content)
}

func TestMigrateLegacyConfigNeverOverwritesAnExistingTarget(t *testing.T) {
	dir := t.TempDir()
	legacy := filepath.Join(dir, "legacy", "config.json")
	target := filepath.Join(dir, "target", "config.json")
	writeFixture(t, legacy, `{"api":{"model":"legacy-model"}}`)
	writeFixture(t, target, `{"api":{"model":"already-here"}}`)
	if err := migrateLegacyConfig(target, legacy); err != nil {
		t.Fatal(err)
	}
	equal(t, readFixture(t, target), `{"api":{"model":"already-here"}}`)
}

func TestMigrateLegacyConfigIsANoopWhenNeitherFileExists(t *testing.T) {
	dir := t.TempDir()
	legacy := filepath.Join(dir, "legacy", "config.json")
	target := filepath.Join(dir, "target", "config.json")
	if err := migrateLegacyConfig(target, legacy); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(target); !os.IsNotExist(err) {
		t.Errorf("stat of missing migration target returned %v", err)
	}
}

func TestLoadEnvOverridesFollowPersistedMigration(t *testing.T) {
	dir := isolateLoadEnv(t)
	path := filepath.Join(dir, "legacy.json")
	writeFixture(t, path, `{
		"api":{"base_url":"http://file/v1","api_key":"file-api","model":"file-chat"},
		"tts":{"model":"file-voice"}
	}`)
	writeFixture(t, filepath.Join(dir, ".env"), "\ufeff"+strings.Join([]string{
		"OPENAI_API_KEY=dotenv-key",
		"OPENAI_API_BASE_URL=http://dotenv/v1",
		"OPENAI_MODEL=dotenv-chat",
		"OPENAI_EMBEDDING_MODEL=dotenv-embed",
		"OPENAI_REASONING_EFFORT=high",
		"TTS_BASE_URL=http://voice/v1",
		"TTS_API_KEY=tts-key",
		"STT_BASE_URL=http://stt/v1",
		"STT_API_KEY=stt-key",
	}, "\n"))
	child := filepath.Join(dir, "nested")
	if err := os.Mkdir(child, 0o700); err != nil {
		t.Fatal(err)
	}
	t.Chdir(child)
	t.Setenv("OPENAI_API_KEY", "process-key")

	cfg, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	deepEqual(t, resolve(t, cfg, TaskTalk), ResolvedLlm{
		ProviderID: "default", BaseURL: "http://dotenv/v1", APIKey: "process-key",
		Model: "dotenv-chat", ReasoningEffort: "high",
	})
	deepEqual(t, resolve(t, cfg, TaskEmbedding), ResolvedLlm{
		ProviderID: "default", BaseURL: "http://dotenv/v1", APIKey: "process-key",
		Model: "dotenv-embed", ReasoningEffort: "high",
	})
	deepEqual(t, resolve(t, cfg, TaskTTS), ResolvedLlm{
		ProviderID: "env-tts", BaseURL: "http://voice/v1", APIKey: "tts-key",
		Model: "file-voice", ReasoningEffort: "high",
	})
	deepEqual(t, resolve(t, cfg, TaskSTT), ResolvedLlm{
		ProviderID: "env-stt", BaseURL: "http://stt/v1", APIKey: "stt-key",
		Model: "dotenv-chat", ReasoningEffort: "high",
	})
	persisted := decode[Config](t, readFixture(t, path))
	equal(t, persisted.API.APIKey, "file-api")
	equal(t, resolve(t, &persisted, TaskTalk).Model, "file-chat")
	equal(t, len(persisted.Providers), 1)
	equal(t, persisted.MigrateLLMConfig(), false)
}

func TestEnvOverridesForRoomProvider(t *testing.T) {
	cfg := modelConfig()
	cfg.DefaultRef = modelRef("room", "room-chat")
	cfg.Providers[1].APIKey = "room-key"
	cfg.addLegacyProvider("env-tts", "Collision", "http://other/v1", "")
	values := map[string]string{
		"OPENAI_API_BASE_URL": "http://chat-bridge/v1",
		"OPENAI_API_KEY":      "api-key",
		"TTS_BASE_URL":        "http://voice-bridge/v1",
		"TTS_API_KEY":         "voice-key",
	}
	cfg.applyEnvOverrides(func(key string) (string, bool) {
		v, ok := values[key]
		return v, ok
	})
	equal(t, cfg.Providers[1].BaseURL, "mist-network://team")
	equal(t, cfg.Providers[1].APIKey, "room-key")
	equal(t, cfg.API.APIKey, "api-key")
	equal(t, cfg.Mist.AIBaseURL, "http://voice-bridge/v1")
	equal(t, cfg.TaskRef(TaskTTS).ProviderID, "env-tts-")
	equal(t, resolve(t, &cfg, TaskTTS).APIKey, "")
	equal(t, resolve(t, &cfg, TaskTalk).APIKey, "")
}

func TestSaveAndExplicitMissingConfig(t *testing.T) {
	dir := isolateLoadEnv(t)
	const relative = "./nested/config.json"
	equal(t, ResolvePath(relative), relative)
	equal(t, ResolvePath(""), filepath.Join(DataDir(), "config.json"))
	writeFixture(t, "config.json", `{"api":{"model":"legacy"}}`)

	cfg, err := Load(relative)
	if err != nil {
		t.Fatal(err)
	}
	equal(t, cfg.Language, "")
	equal(t, cfg.API.Model, "")
	equal(t, cfg.Providers[0].ID, "default")
	if _, err := os.Stat(relative); !os.IsNotExist(err) {
		t.Errorf("explicit missing config unexpectedly written: stat error %v", err)
	}

	cfg.Providers[0].APIKey = "file-key"
	if err := cfg.Save(relative); err != nil {
		t.Fatal(err)
	}
	data := readFixture(t, filepath.Join(dir, "nested", "config.json"))
	if !strings.Contains(data, "\n  \"language\": \"\"") {
		t.Error("saved JSON was not pretty-printed")
	}
	deepEqual(t, decode[Config](t, data), *cfg)
}
