package translate

import (
	"fmt"
	"slices"
	"testing"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
)

func testSettings() config.TranslationConfig {
	settings := config.DefaultTranslationConfig()
	settings.ModeValue = "interpret"
	return settings
}

func TestLiveRefsPreserveClientProviderAndUseEffectiveFallback(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.Providers = []config.ProviderConfig{
		{ID: "http", BaseURL: "http://endpoint/v1", Enabled: true},
		{ID: "room", BaseURL: "mist-network://team", Enabled: false},
	}
	cfg.DefaultRef = &config.ModelRef{ProviderID: "http", Model: "default-model"}
	roomRef := &config.ModelRef{ProviderID: "room", Model: "remote-model"}
	cfg.Translation.ModelRef = roomRef
	engine, err := newEngine(bus.New(), &cfg)
	if err != nil {
		t.Fatal(err)
	}
	if engine.providerID != "http" {
		t.Fatalf("provider = %q, want http", engine.providerID)
	}
	checkModel := func(engine *translationEngine, want string) {
		t.Helper()
		model, err := engine.resolveModel(cfg.Translation)
		if err != nil || model != want {
			t.Fatalf("resolveModel() = %q, %v, want %q", model, err, want)
		}
	}
	checkModel(engine, "default-model")
	cfg.Translation.ModelRef = &config.ModelRef{ProviderID: "http", Model: "new-model"}
	checkModel(engine, "new-model")
	cfg.Translation.ModelRef = &config.ModelRef{ProviderID: "missing", Model: "new-model"}
	if _, err := engine.resolveModel(cfg.Translation); err == nil {
		t.Fatal("resolveModel() accepted a missing provider")
	}

	cfg.Providers[1].Enabled = true
	cfg.Translation.ModelRef = nil
	engine, err = newEngine(bus.New(), &cfg)
	if err != nil {
		t.Fatal(err)
	}
	cfg.Translation.ModelRef = roomRef
	checkModel(engine, "default-model")
}

func TestHistoryKeepsConfiguredWindow(t *testing.T) {
	var history []string
	for i := range 5 {
		history = pushHistory(history, fmt.Sprintf("m%d", i), 3)
	}
	if want := []string{"m2", "m3", "m4"}; !slices.Equal(history, want) {
		t.Fatalf("history = %q, want %q", history, want)
	}
}

func TestZeroContextSizeKeepsCurrentUtterance(t *testing.T) {
	history := pushHistory(nil, "a", 0)
	history = pushHistory(history, "b", 0)
	if want := []string{"b"}; !slices.Equal(history, want) {
		t.Fatalf("history = %q, want %q", history, want)
	}
	if got := buildContext(history); got != "" {
		t.Fatalf("context = %q, want empty", got)
	}
}

func TestBuildContext(t *testing.T) {
	for _, tc := range []struct {
		name    string
		history []string
		want    string
	}{
		{"excludes current utterance", []string{"one", "two", "three"}, "Context for translation:\nPrevious message: one\nPrevious message: two\n\n"},
		{"first utterance", []string{"only"}, ""},
		{"empty history", nil, ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := buildContext(tc.history); got != tc.want {
				t.Errorf("buildContext() = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestBuildPrompt(t *testing.T) {
	const want = "Translate the following text to 英語 accurately and concisely, using the provided context for reference but translating ONLY the final text. Return ONLY the translated text.\n\nContext for translation:\nPrevious message: x\n\nText to translate: こんにちは"
	if got := buildPrompt("英語", "Context for translation:\nPrevious message: x\n\n", "こんにちは"); got != want {
		t.Errorf("buildPrompt() = %q, want %q", got, want)
	}
}

func TestResolveTargets(t *testing.T) {
	for _, tc := range []struct {
		name     string
		source   source
		text     string
		primary  string
		second   string
		auto     bool
		want     []string
		reversed bool
	}{
		{"configured languages", sourceUser, "こんにちは", "英語", "中国語", true, []string{"英語", "中国語"}, false},
		{"reverse target language", sourceUser, "hello there", "英語", "", true, []string{"日本語"}, true},
		{"foreign language outside targets", sourceUser, "hello there", "中国語", "", true, []string{"中国語"}, false},
		{"auto reverse disabled", sourceUser, "hello there", "英語", "", false, []string{"英語"}, false},
		{"agent replies never reverse", sourceAgent, "hello there", "英語", "", true, []string{"英語"}, false},
		{"reverse second target", sourceUser, "hello there", "中国語", "英語", true, []string{"日本語"}, true},
		{"trim and deduplicate targets", sourceUser, "こんにちは", " 英語 ", "英語", true, []string{"英語"}, false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			settings := testSettings()
			settings.TargetLanguage, settings.TargetLanguage2 = tc.primary, tc.second
			settings.AutoReverse = tc.auto
			langs, reversed := resolveTargets(settings, tc.source, tc.text)
			if !slices.Equal(langs, tc.want) || reversed != tc.reversed {
				t.Errorf("resolveTargets() = %q, %t, want %q, %t", langs, reversed, tc.want, tc.reversed)
			}
		})
	}
}

func TestChatboxListsOriginalThenArrivedTranslationsInTargetOrder(t *testing.T) {
	langs := []string{"英語", "中国語"}
	results := map[string]string{"中国語": "你好"}
	if got, want := composeChatbox("こんにちは", langs, results), "こんにちは\n你好"; got != want {
		t.Errorf("composeChatbox() = %q, want %q", got, want)
	}
	results["英語"] = "Hello"
	if got, want := composeChatbox("こんにちは", langs, results), "こんにちは\nHello\n你好"; got != want {
		t.Errorf("composeChatbox() = %q, want %q", got, want)
	}
}
