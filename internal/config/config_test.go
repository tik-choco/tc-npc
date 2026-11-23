package config

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

func decode[T any](t *testing.T, data string) T {
	t.Helper()
	var v T
	if err := json.Unmarshal([]byte(data), &v); err != nil {
		t.Fatal(err)
	}
	return v
}

func encode(t *testing.T, value any) string {
	t.Helper()
	data, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func equal[T comparable](t *testing.T, got, want T) {
	t.Helper()
	if got != want {
		t.Errorf("got %v, want %v", got, want)
	}
}

func deepEqual(t *testing.T, got, want any) {
	t.Helper()
	if !reflect.DeepEqual(got, want) {
		t.Errorf("got %#v, want %#v", got, want)
	}
}

func writeFixture(t *testing.T, path, data string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(data), 0o600); err != nil {
		t.Fatal(err)
	}
}

func readFixture(t *testing.T, path string) string {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func exampleJSON(t *testing.T) string {
	t.Helper()
	return readFixture(t, filepath.Join("..", "..", "config.example.json"))
}

func isolateLoadEnv(t *testing.T) string {
	t.Helper()
	for _, name := range []string{
		"OPENAI_API_KEY", "OPENAI_API_BASE_URL", "OPENAI_MODEL",
		"OPENAI_EMBEDDING_MODEL", "OPENAI_REASONING_EFFORT",
		"TTS_BASE_URL", "TTS_API_KEY", "STT_BASE_URL", "STT_API_KEY",
	} {
		t.Setenv(name, "")
		if err := os.Unsetenv(name); err != nil {
			t.Fatal(err)
		}
	}
	dir := t.TempDir()
	writeFixture(t, filepath.Join(dir, ".env"), "")
	t.Chdir(dir)
	return dir
}

func TestAvatarLinkDefaultsInOlderMistConfigs(t *testing.T) {
	config := decode[Config](t, `{"mist":{"enabled":true}}`)
	link := config.Mist.AvatarLink
	equal(t, link.Enabled, false)
	equal(t, link.RoomID, "tc-avatar-link-v1")
	equal(t, link.Name, "")
	equal(t, len(link.AllowHosts), 0)

	config = decode[Config](t, `{"mist":{"avatar_link":{"enabled":true}}}`)
	equal(t, config.Mist.AvatarLink.Enabled, true)
	equal(t, config.Mist.AvatarLink.RoomID, "tc-avatar-link-v1")
}

func TestDefaultsRoundTrip(t *testing.T) {
	config := DefaultConfig()
	equal(t, config.API.BaseURL, "http://localhost:11434/v1")
	equal(t, config.Talk.Enabled, true)
	equal(t, config.Server.Addr, "127.0.0.1:47950")
	deepEqual(t, decode[Config](t, encode(t, config)), config)
}

func TestRustDefaultJSON(t *testing.T) {
	// This fixture was emitted by serde_json from Config::default().
	want := decode[map[string]any](t, readFixture(t, filepath.Join("testdata", "rust-default.json")))
	deepEqual(t, decode[map[string]any](t, encode(t, DefaultConfig())), want)
	want["language"] = "auto"
	deepEqual(t, decode[map[string]any](t, encode(t, decode[Config](t, `{}`))), want)
}

func TestConfigWireFieldRules(t *testing.T) {
	cfg := decode[Config](t, `{
		"Language":"wrong case",
		"talk":{"enabled":false,"history_size":0,"model_ref":null,"preset_id":"import"},
		"providers":[{"id":"empty","enabled":false,"models_fetched_at":null}],
		"mist":{"instance":"","state_dir":null},
		"action":{"routes":[["patrol",true]]}
	}`)
	equal(t, cfg.Language, "auto")
	equal(t, cfg.Talk.Enabled, false)
	equal(t, cfg.Talk.HistorySize, 0)
	equal(t, cfg.Talk.PresetID, "import")
	deepEqual(t, cfg.Action.Routes, []RouteConfig{{Name: "patrol", Loop: true, Waypoints: []WaypointConfig{}}})
	output := decode[map[string]any](t, encode(t, cfg))
	talk := output["talk"].(map[string]any)
	_, hasModelRef := talk["model_ref"]
	_, hasPresetID := talk["preset_id"]
	equal(t, hasModelRef, false)
	equal(t, hasPresetID, false)
	mist := output["mist"].(map[string]any)
	equal(t, mist["instance"], any(""))
	_, hasStateDir := mist["state_dir"]
	equal(t, hasStateDir, false)
	provider := output["providers"].([]any)[0].(map[string]any)
	equal(t, provider["enabled"], any(false))
	stamp, hasStamp := provider["models_fetched_at"]
	equal(t, hasStamp, true)
	equal(t, stamp, nil)
	deepEqual(t, provider["models"], []any{})
	deepEqual(t, provider["shared"], []any{})

	for _, data := range []string{
		`{"providers":null}`,
		`{"providers":[{"models":[null]}]}`,
		`{"default_ref":{"model":"missing provider id"}}`,
		`{"talk":{"enabled":false,"enabled":true}}`,
		`{"action":{"routes":[{}]}}`,
	} {
		t.Run(data, func(t *testing.T) {
			var cfg Config
			if err := json.Unmarshal([]byte(data), &cfg); err == nil {
				t.Error("invalid config parsed successfully")
			}
		})
	}
}

func TestAffectConfigDefaultsToEnabledForAnOldConfig(t *testing.T) {
	for _, config := range []Config{
		decode[Config](t, `{"talk":{"enabled":true}}`),
		DefaultConfig(),
	} {
		equal(t, config.Talk.StyleRules, true)
		equal(t, config.Talk.AllowSilence, true)
		equal(t, config.Talk.Affect.Enabled, true)
		equal(t, config.Talk.Affect.ForcedClosure, true)
	}
}

func TestParsesPartialJSON(t *testing.T) {
	config := decode[Config](t, `{"api":{"model":"gpt-4o-mini"}}`)
	equal(t, config.API.Model, "gpt-4o-mini")
	equal(t, config.API.BaseURL, "http://localhost:11434/v1")
	equal(t, config.API.ReasoningEffort, "none")
}

func TestLanguageDefaultsToAutoAndInjectsNothing(t *testing.T) {
	config := decode[Config](t, `{"api":{"model":"m"}}`)
	equal(t, config.Language, "auto")
	_, ok := LanguageInstruction(config.Language)
	equal(t, ok, false)
	defaults := DefaultConfig()
	equal(t, defaults.Language, "")
	_, ok = LanguageInstruction(defaults.Language)
	equal(t, ok, false)
}

func TestLanguageInstructionCoversTheThreeSupportedLanguages(t *testing.T) {
	for _, tc := range []struct{ language, want string }{
		{"ja", "Japanese"}, {"EN", "English"}, {" zh ", "Chinese"},
	} {
		t.Run(tc.language, func(t *testing.T) {
			instruction, ok := LanguageInstruction(tc.language)
			if !ok || !strings.Contains(instruction, tc.want) {
				t.Errorf("instruction %q does not contain %s", instruction, tc.want)
			}
		})
	}
	_, ok := LanguageInstruction("klingon")
	equal(t, ok, false)
}

func TestWithLanguageInstructionAppendsOrPassesThrough(t *testing.T) {
	equal(t, WithLanguageInstruction("Describe it.", "auto"), "Describe it.")
	equal(t, WithLanguageInstruction("Describe it.", "en"), "Describe it.\n\nAlways write your reply in English.")
	equal(t, WithLanguageInstruction("  ", "en"), "Always write your reply in English.")
}

func TestAnnouncementActionsDefaultToEmptyAndParseByKind(t *testing.T) {
	ann := decode[AnnouncementConfig](t, `{"time":"09:00","text":"hi"}`)
	equal(t, len(ann.Actions), 0)

	ann = decode[AnnouncementConfig](t, `{
		"time":"17:00", "text":"",
		"actions":[
			{"kind":"action","content":"原点に移動して"},
			{"kind":"command","text":"route patrol"},
			{"kind":"chat","content":"今日の予定は?"},
			{"kind":"speak","content":"こんにちは","chime_file":"c.wav"},
			{"kind":"suspend"},
			{"kind":"resume"},
			{"kind":"raw","topic":"agent:interrupt","type":"resume","payload":{}}
		]
	}`)
	deepEqual(t, ann.Actions, []ScheduledAction{
		{Kind: ScheduledActionAction, Content: "原点に移動して"},
		{Kind: ScheduledActionCommand, Text: "route patrol"},
		{Kind: ScheduledActionChat, Content: "今日の予定は?"},
		{Kind: ScheduledActionSpeak, Content: "こんにちは", ChimeFile: "c.wav"},
		{Kind: ScheduledActionSuspend},
		{Kind: ScheduledActionResume},
		{Kind: ScheduledActionRaw, Topic: "agent:interrupt", MessageType: "resume", Payload: json.RawMessage("{}")},
	})
}

func TestScheduledActionRoundTripsThroughJSON(t *testing.T) {
	action := ScheduledAction{Kind: ScheduledActionCommand, Text: "go home"}
	data := encode(t, action)
	doc := decode[map[string]any](t, data)
	equal(t, doc["kind"], any("command"))
	equal(t, doc["text"], any("go home"))
	deepEqual(t, decode[ScheduledAction](t, data), action)
}

func TestScheduledActionWireShape(t *testing.T) {
	for _, tc := range []struct{ input, output string }{
		{`{"kind":"speak"}`, `{"kind":"speak","content":"","chime_file":""}`},
		{`{"kind":"action"}`, `{"kind":"action","content":""}`},
		{`{"kind":"chat"}`, `{"kind":"chat","content":""}`},
		{`{"kind":"command"}`, `{"kind":"command","text":""}`},
		{`{"kind":"suspend","content":"unused"}`, `{"kind":"suspend"}`},
		{`{"kind":"resume"}`, `{"kind":"resume"}`},
		{`{"kind":"raw","topic":"t","type":"m"}`, `{"kind":"raw","topic":"t","type":"m","payload":null}`},
	} {
		t.Run(tc.input, func(t *testing.T) {
			equal(t, encode(t, decode[ScheduledAction](t, tc.input)), tc.output)
		})
	}
	for _, data := range []string{
		`{"kind":"other"}`, `{"kind":"raw","topic":"t"}`,
		`{"kind":"speak","content":null}`, `{"kind":"speak","kind":"speak"}`,
	} {
		var action ScheduledAction
		if err := json.Unmarshal([]byte(data), &action); err == nil {
			t.Errorf("invalid action %s parsed successfully", data)
		}
	}
}

func TestAnnouncementEffectiveSettings(t *testing.T) {
	defaults := DefaultSchedulerDefaults()
	defaults.ChimeFile = "default.wav"
	ann := AnnouncementConfig{Volume: -1}
	equal(t, ann.EffectiveChimeFile(&defaults), "default.wav")
	equal(t, ann.EffectiveVolume(&defaults), 1)
	equal(t, ann.EffectiveBGMVolume(&defaults), 0.5)

	ann.ChimeFile, ann.Volume, ann.BGMVolume = "local.wav", 0.5, 1
	equal(t, ann.EffectiveChimeFile(&defaults), "local.wav")
	equal(t, ann.EffectiveVolume(&defaults), 0.5)
	equal(t, ann.EffectiveBGMVolume(&defaults), 1)
}

func TestTranslationDefaultsToOffForAnOldConfig(t *testing.T) {
	config := decode[Config](t, `{"api":{"model":"m"}}`)
	equal(t, config.Translation.Mode(), TranslationModeOff)
	equal(t, config.Translation.SuppressesChat(), false)
	defaults := DefaultConfig()
	equal(t, defaults.Translation.Mode(), TranslationModeOff)
}

func TestTranslationModeParsesLeniently(t *testing.T) {
	for _, tc := range []struct {
		raw        string
		mode       TranslationMode
		suppresses bool
	}{
		{" Interpret ", TranslationModeInterpret, true},
		{"assist", TranslationModeAssist, false},
		{"nonsense", TranslationModeOff, false},
	} {
		t.Run(tc.raw, func(t *testing.T) {
			translation := DefaultTranslationConfig()
			translation.ModeValue = tc.raw
			equal(t, translation.Mode(), tc.mode)
			equal(t, translation.SuppressesChat(), tc.suppresses)
			equal(t, translation.Mode().String(), string(tc.mode))
			equal(t, translation.Mode().IsOff(), tc.mode == TranslationModeOff)
		})
	}
}

func TestTranslationScopeDefaultsToBothForAConfigPredatingIt(t *testing.T) {
	config := decode[Config](t, `{"translation":{"mode":"assist"}}`)
	equal(t, config.Translation.Scope(), TranslationScopeBoth)
	equal(t, config.Translation.Scope().TranslatesInput(), true)
	equal(t, config.Translation.Scope().TranslatesOutput(), true)
	defaults := DefaultConfig()
	equal(t, defaults.Translation.Scope(), TranslationScopeBoth)
}

func TestTranslationScopeParsesLenientlyAndGatesOneHalfEach(t *testing.T) {
	for _, tc := range []struct {
		raw           string
		scope         TranslationScope
		input, output bool
	}{
		{" Input ", TranslationScopeInput, true, false},
		{"output", TranslationScopeOutput, false, true},
		{"nonsense", TranslationScopeBoth, true, true},
	} {
		t.Run(tc.raw, func(t *testing.T) {
			translation := DefaultTranslationConfig()
			translation.ScopeValue = tc.raw
			equal(t, translation.Scope(), tc.scope)
			equal(t, translation.Scope().TranslatesInput(), tc.input)
			equal(t, translation.Scope().TranslatesOutput(), tc.output)
			equal(t, translation.Scope().String(), string(tc.scope))
		})
	}
}

func TestTranslationScopeDoesNotDecideWhetherNPCTalkAnswers(t *testing.T) {
	translation := DefaultTranslationConfig()
	translation.ModeValue = "interpret"
	for _, scope := range []string{"input", "output", "both"} {
		t.Run(scope, func(t *testing.T) {
			translation.ScopeValue = scope
			equal(t, translation.SuppressesChat(), true)
		})
	}
}

func TestTargetLanguagesDropBlanksAndDuplicates(t *testing.T) {
	translation := DefaultTranslationConfig()
	translation.TargetLanguage = "英語"
	for _, tc := range []struct {
		second string
		want   []string
	}{
		{"", []string{"英語"}},
		{"中国語", []string{"英語", "中国語"}},
		{" 英語 ", []string{"英語"}},
	} {
		translation.TargetLanguage2 = tc.second
		deepEqual(t, translation.TargetLanguages(), tc.want)
	}
}

func TestRedactsAPIKeys(t *testing.T) {
	config := DefaultConfig()
	config.API.APIKey = "DUMMY-api-key"
	redacted, err := config.RedactedJSON()
	if err != nil {
		t.Fatal(err)
	}
	equal(t, redacted["api"].(map[string]any)["api_key"], any("***"))
	equal(t, config.API.APIKey, "DUMMY-api-key")
}

func TestRedactsProviderAPIKeysToo(t *testing.T) {
	config := DefaultConfig()
	p := DefaultProviderConfig()
	p.ID, p.Label, p.BaseURL, p.APIKey = "p1", "P1", "http://x", "DUMMY-provider-key"
	config.Providers = append(config.Providers, p)
	redacted, err := config.RedactedJSON()
	if err != nil {
		t.Fatal(err)
	}
	provider := redacted["providers"].([]any)[0].(map[string]any)
	equal(t, provider["api_key"], any("***"))
}

func TestRedactionPreservesJSONNumbersAndMasksNestedPayload(t *testing.T) {
	cfg := decode[Config](t, `{
		"scheduler":{"announcements":[{"time":"12:00","text":"","actions":[
			{"kind":"raw","topic":"t","type":"m","payload":[{"api_key":"DUMMY-raw-key","n":18446744073709551615}]}
		]}]}
	}`)
	doc, err := cfg.RedactedJSON()
	if err != nil {
		t.Fatal(err)
	}
	result := encode(t, doc)
	for _, fragment := range []string{
		`"api_key":"***"`, `"n":18446744073709551615`,
		`"threshold":0.10000000149011612`, `"input_threshold":0.009999999776482582`,
	} {
		if !strings.Contains(result, fragment) {
			t.Errorf("redacted JSON is missing %s", fragment)
		}
	}
	if strings.Contains(result, "DUMMY-raw-key") {
		t.Error("nested API key leaked in redacted JSON")
	}
}

func TestUnmaskProviderKeysRestoresByIDNotPosition(t *testing.T) {
	current := DefaultConfig()
	current.addLegacyProvider("p1", "P1", "http://p1", "real-p1-key")
	current.addLegacyProvider("p2", "P2", "http://p2", "real-p2-key")
	incoming := decode[map[string]any](t, `{
		"providers":[
			{"id":"p2","label":"P2","base_url":"http://p2","api_key":"***"},
			{"id":"p1","label":"P1","base_url":"http://p1","api_key":"***"}
		]
	}`)
	UnmaskProviderKeys(incoming, &current)
	providers := incoming["providers"].([]any)
	equal(t, providers[0].(map[string]any)["api_key"], any("real-p2-key"))
	equal(t, providers[1].(map[string]any)["api_key"], any("real-p1-key"))
}

func TestUnmaskProviderKeysLeavesUnmaskedKeysUntouched(t *testing.T) {
	current := DefaultConfig()
	incoming := decode[map[string]any](t, `{
		"providers":[{"id":"new","label":"New","base_url":"http://new","api_key":"freshly-typed-key"}]
	}`)
	UnmaskProviderKeys(incoming, &current)
	equal(t, incoming["providers"].([]any)[0].(map[string]any)["api_key"], any("freshly-typed-key"))
}

func TestExampleJSONRoundTrip(t *testing.T) {
	source := exampleJSON(t)
	dir := isolateLoadEnv(t)
	path := filepath.Join(dir, "config.example.json")
	writeFixture(t, path, source)
	cfg, err := Load(path)
	if err != nil {
		t.Fatal(err)
	}
	wire := encode(t, cfg)
	roundTrip := decode[Config](t, wire)
	deepEqual(t, &roundTrip, cfg)
	equal(t, encode(t, roundTrip), wire)
	equal(t, readFixture(t, path), source)
}
