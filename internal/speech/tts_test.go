package speech

import (
	"context"
	"log/slog"
	"slices"
	"strings"
	"sync/atomic"
	"testing"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/module"
)

func TestSplitSentences(t *testing.T) {
	long := strings.Repeat("a", 5000)
	for _, tc := range []struct {
		name, text string
		want       []string
	}{
		{"Japanese punctuation", "こんにちは。元気ですか？うん！", []string{"こんにちは。", "元気ですか？", "うん！"}},
		{"ASCII punctuation", "How are you? I'm great! Really?", []string{"How are you?", "I'm great!", "Really?"}},
		{"ASCII period", "Hello there. How are you today.", []string{"Hello there. How are you today."}},
		{"bare newlines", "line one\nline two\nline three", []string{"line one", "line two", "line three"}},
		{"no terminator", "no terminator anywhere in this reply", []string{"no terminator anywhere in this reply"}},
		{"empty", "", nil},
		{"whitespace", "   \n\t  ", nil},
		{"terminator run", "えっ!?", []string{"えっ!", "?"}},
		{"closing quote", "そう言った。\u300d", []string{"そう言った。", "\u300d"}},
		{"blank lines", "最初の文。\n\n次の文。", []string{"最初の文。", "次の文。"}},
		{"long unterminated sentence", long, []string{long}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := splitSentences(tc.text); !slices.Equal(got, tc.want) {
				t.Errorf("sentences = %q, want %q", got, tc.want)
			}
		})
	}
}

func TestTruncateForTTS(t *testing.T) {
	long := strings.Repeat("a", 250)
	for _, tc := range []struct {
		text string
		max  uint32
		want string
	}{
		{long, 200, long[:200] + "..."},
		{"hello", 200, "hello"},
		{"こんにちは", 3, "こんに..."},
		{"hello", 0, "hello"},
	} {
		if got := truncateForTTS(tc.text, tc.max); got != tc.want {
			t.Errorf("truncate(%q, %d) = %q, want %q", tc.text, tc.max, got, tc.want)
		}
	}
}

func TestResolveVoice(t *testing.T) {
	name, model, empty := "shimmer", "tts-1-hd", ""
	for _, tc := range []struct {
		name              string
		character         *character.Character
		voice, voiceModel string
	}{
		{"no active character", nil, "alloy", "tts-1"},
		{"both fields", &character.Character{VoiceName: &name, VoiceModel: &model}, name, model},
		{"voice only", &character.Character{VoiceName: &name}, name, "tts-1"},
		{"model only", &character.Character{VoiceModel: &model}, "alloy", model},
		{"empty fields", &character.Character{VoiceName: &empty, VoiceModel: &empty}, "alloy", "tts-1"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if voice, model := resolveVoice(tc.character, "alloy", "tts-1"); voice != tc.voice || model != tc.voiceModel {
				t.Errorf("voice/model = %q/%q, want %q/%q", voice, model, tc.voice, tc.voiceModel)
			}
		})
	}
}

type warningCounter struct {
	slog.Handler
	count atomic.Int64
}

func (c *warningCounter) Handle(_ context.Context, record slog.Record) error {
	if record.Level >= slog.LevelWarn {
		c.count.Add(1)
	}
	return nil
}

func testTTSSession(t *testing.T, enabled bool) *session {
	t.Helper()
	cfg := config.DefaultConfig()
	cfg.TTS.Enabled = enabled
	// A missing provider exposes synthesis attempts without using the network.
	cfg.TTS.ModelRef = &config.ModelRef{ProviderID: "missing", Model: "tts-test"}
	return newSession(t.Context(), module.Ctx{Bus: bus.New(), Config: &cfg, DataDir: t.TempDir()}, speechModule{})
}

func setTTSEnabled(s *session, enabled bool) {
	cfg := *s.cfg
	cfg.TTS.Enabled = enabled
	s.updateConfig(&cfg)
}

func TestDisabledTTSSkipsSynthesisAndPriorityWithoutWarnings(t *testing.T) {
	oldLogger := slog.Default()
	counter := &warningCounter{Handler: oldLogger.Handler()}
	slog.SetDefault(slog.New(counter))
	t.Cleanup(func() { slog.SetDefault(oldLogger) })
	s := testTTSSession(t, false)
	rx := s.bus.Subscribe()
	defer rx.Close()
	if _, ok := synthesizeTTS(t.Context(), s.cfg, "hello", nil); ok {
		t.Fatal("disabled TTS synthesized audio")
	}
	s.speakReply(t.Context(), s.cfg, nil, "hello!")
	s.playAnnouncement(t.Context(), s.cfg, nil, announcement{content: "hello", chimeFile: "missing.wav", volume: 1})
	s.handleMessage(bus.Message{Topic: bus.TopicInterrupt, Type: bus.MsgTTS, Payload: []byte(`{"content":"hello","chime_file":"missing.wav"}`)})
	s.tasks.Wait()
	if counter.count.Load() != 0 || len(rx.C) != 0 {
		t.Fatal("disabled TTS logged a warning or announced an audible line")
	}
	setTTSEnabled(s, true)
	if _, ok := synthesizeTTS(t.Context(), s.cfg, "hello", nil); ok || counter.count.Load() != 1 {
		t.Fatal("enabled TTS did not try to resolve its missing provider")
	}
}

func TestChatResponseTTSFollowsLiveConfigAndVoiceGate(t *testing.T) {
	s := testTTSSession(t, false)
	rx := s.bus.Subscribe()
	defer rx.Close()
	for _, tc := range []struct{ enabled, voice bool }{
		{false, true}, {true, true}, {false, true}, {true, true}, {true, false},
	} {
		setTTSEnabled(s, tc.enabled)
		s.setVoice(tc.voice)
		s.handleMessage(bus.Message{Topic: bus.TopicChat, Type: bus.MsgChatResponse, Payload: []byte(`{"content":"hello!"}`)})
		s.tasks.Wait()
		select {
		case msg := <-rx.C:
			if !tc.enabled || !tc.voice || msg.Topic != bus.TopicUI || msg.Type != UIMsgTTSLine || msg.Str("text") != "hello!" {
				t.Fatalf("unexpected TTS line with enabled=%v voice=%v: %#v", tc.enabled, tc.voice, msg)
			}
		default:
			if tc.enabled && tc.voice {
				t.Fatal("enabled voice did not announce the sentence")
			}
		}
	}
}

func TestDisablingTTSCancelsOldReplyEvenAfterReenable(t *testing.T) {
	s := testTTSSession(t, true)
	rx := s.bus.Subscribe()
	defer rx.Close()
	oldConfig := s.cfg
	oldReply, cancel := context.WithCancel(s.ttsCtx)
	defer cancel()
	s.cancelReply = cancel
	clipCtx, cancelClip := context.WithCancel(t.Context())
	defer cancelClip()
	s.playback = &playbackWorker{cancelClip: cancelClip, queue: []*playbackClip{{wav: []byte("queued")}}}
	setTTSEnabled(s, false)
	if oldReply.Err() == nil || clipCtx.Err() == nil || len(s.playback.queue) != 0 {
		t.Fatal("disabling TTS did not cancel synthesis, playback, and queued clips")
	}
	s.speakReply(oldReply, oldConfig, nil, "old!")
	setTTSEnabled(s, true)
	s.speakReply(oldReply, oldConfig, nil, "old!")
	if len(rx.C) != 0 || len(s.playback.queue) != 0 {
		t.Fatal("stale reply announced a line or queued audio")
	}
	current := s.ttsCtx
	setTTSEnabled(s, true)
	if s.ttsCtx != current || current.Err() != nil {
		t.Fatal("unrelated config update invalidated the enabled TTS context")
	}
}
