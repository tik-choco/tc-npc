package talk

import (
	"context"
	"encoding/json"
	"errors"
	"testing"
	"testing/synctest"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/module"
)

func configUpdated(t *testing.T, cfg config.Config) bus.Message {
	t.Helper()
	payload, err := json.Marshal(cfg)
	if err != nil {
		t.Fatal(err)
	}
	return bus.Message{Topic: bus.TopicConfig, Type: bus.MsgConfigUpdated, Payload: payload}
}

func TestConfigUpdateTogglesInterpretation(t *testing.T) {
	e := testEngine(config.DefaultTalkConfig())
	var state talkState
	cfg := config.DefaultConfig()
	speech := bus.Message{Topic: bus.TopicSense, Type: bus.MsgSpeech, Payload: json.RawMessage(`"hi"`)}
	for _, tc := range []struct {
		mode       string
		suppressed bool
	}{{"interpret", true}, {"assist", false}} {
		cfg.Translation.ModeValue = tc.mode
		handleBusMessage(e, &state, configUpdated(t, cfg), "")
		action := handleBusMessage(e, &state, speech, "")
		if state.interpretMode != tc.suppressed || (action.speech == nil) != tc.suppressed {
			t.Errorf("mode %q: interpret = %v, speech queued = %v", tc.mode, state.interpretMode, action.speech != nil)
		}
	}
}

func TestBrokenConfigKeepsCurrentMode(t *testing.T) {
	e := testEngine(config.DefaultTalkConfig())
	state := talkState{interpretMode: true}
	broken := bus.Message{Topic: bus.TopicConfig, Type: bus.MsgConfigUpdated, Payload: json.RawMessage(`"not a config"`)}
	handleBusMessage(e, &state, broken, "")
	if !state.interpretMode {
		t.Fatal("broken config cleared interpretation mode")
	}
}

func TestSuspendAndResumeGateSpeech(t *testing.T) {
	e := testEngine(config.DefaultTalkConfig())
	var state talkState
	interrupt := bus.Message{Topic: bus.TopicInterrupt, Type: bus.MsgSuspend}
	speech := bus.Message{Topic: bus.TopicSense, Type: bus.MsgSpeech, Payload: json.RawMessage(`"hi"`)}
	handleBusMessage(e, &state, interrupt, "")
	if !state.suspended || state.resumeDeadline.IsZero() || handleBusMessage(e, &state, speech, "").speech != nil {
		t.Fatalf("suspend did not gate speech: %+v", state)
	}
	interrupt.Type = bus.MsgResume
	handleBusMessage(e, &state, interrupt, "")
	if state.suspended || !state.resumeDeadline.IsZero() || handleBusMessage(e, &state, speech, "").speech == nil {
		t.Fatalf("resume did not restore speech: %+v", state)
	}
}

func TestSpeechPayloadFields(t *testing.T) {
	e := testEngine(config.DefaultTalkConfig())
	for _, tc := range []struct {
		name, payload      string
		speaker, requestID *string
		transliterate      bool
	}{
		{"with fields", `{"content":"hi","speaker":"太郎","request_id":"req-123","transliterate_name":true}`, new("太郎"), new("req-123"), true},
		{"without fields", `{"content":"hi"}`, nil, nil, false},
		{"bare string", `"hi"`, nil, nil, false},
		{"explicit false", `{"content":"hi","transliterate_name":false}`, nil, nil, false},
		{"empty requester", `{"content":"hi","request_id":""}`, nil, new(""), false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			msg := bus.Message{Topic: bus.TopicSense, Type: bus.MsgSpeech, Payload: json.RawMessage(tc.payload)}
			input := handleBusMessage(e, &talkState{}, msg, "").speech
			if input == nil || input.content != "hi" {
				t.Fatalf("speech input = %+v", input)
			}
			for _, field := range []struct {
				name      string
				got, want *string
			}{
				{"speaker", input.speaker, tc.speaker},
				{"request_id", input.requestID, tc.requestID},
			} {
				if (field.got == nil) != (field.want == nil) || stringOrEmpty(field.got) != stringOrEmpty(field.want) {
					t.Errorf("%s = %v, want %v", field.name, field.got, field.want)
				}
			}
			if input.transliterate != tc.transliterate {
				t.Errorf("transliterate flag = %v, want %v", input.transliterate, tc.transliterate)
			}
		})
	}
}

func TestPersonMemoryDispatch(t *testing.T) {
	e := testEngine(config.DefaultTalkConfig())
	msg := bus.Message{
		Topic: bus.TopicMem, Type: bus.MsgPersonMemory,
		Payload: json.RawMessage(`{"person_id":"abc","name":"太郎","content":"太郎について覚えていること:\n- 犬を飼っている"}`),
	}
	action := handleBusMessage(e, &talkState{}, msg, "")
	if action.update == nil {
		t.Fatal("person memory was not dispatched")
	}
	action.update()
	if got := e.state.personMemory; got != "太郎について覚えていること:\n- 犬を飼っている" {
		t.Errorf("person memory = %q", got)
	}
}

func TestActiveCharacterIDTracksChanges(t *testing.T) {
	e := testEngine(config.DefaultTalkConfig())
	var state talkState
	cfg := config.DefaultConfig()
	for _, tc := range []struct {
		id      string
		changed bool
	}{{"", false}, {"abc", true}, {"abc", false}, {"", true}} {
		cfg.Character.ActiveID = tc.id
		action := handleBusMessage(e, &state, configUpdated(t, cfg), "")
		if state.activeCharacterID != tc.id || (action.update != nil) != tc.changed {
			t.Errorf("active character %q: tracked = %q, reload = %v", tc.id, state.activeCharacterID, action.update != nil)
		}
	}
}

func TestCharacterSwitchInstallsAndClearsPersona(t *testing.T) {
	dataDir := t.TempDir()
	alice := character.Character{ID: "alice", Sheet: character.CharacterSheet{Name: "アリス", Summary: "元気な図書委員"}}
	if err := character.SaveCharacter(dataDir, &alice); err != nil {
		t.Fatal(err)
	}
	e := testEngine(config.DefaultTalkConfig())
	if e.state.persona != nil {
		t.Fatal("initial persona should be empty")
	}
	if _, err := e.Chat(t.Context(), "じゃあね"); err != nil {
		t.Fatal(err)
	}
	familiarity := e.state.affect.state().Familiarity()

	var state talkState
	cfg := config.DefaultConfig()
	for _, tc := range []struct{ id, persona string }{
		{"alice", character.PersonaPrompt(&alice.Sheet)},
		{"missing", ""},
		{"alice", character.PersonaPrompt(&alice.Sheet)},
		{"", ""},
	} {
		cfg.Character.ActiveID = tc.id
		action := handleBusMessage(e, &state, configUpdated(t, cfg), dataDir)
		if action.update == nil || state.activeCharacterID != tc.id {
			t.Fatalf("active character %q was not queued", tc.id)
		}
		action.update()
		if got := e.state.persona; stringOrEmpty(got) != tc.persona || (got == nil) != (tc.persona == "") {
			t.Errorf("active character %q: persona = %q, want %q", tc.id, stringOrEmpty(got), tc.persona)
		}
		if len(e.state.history) != 2 || e.state.affect.state().Familiarity() != familiarity {
			t.Fatal("character switch reset conversation state")
		}
	}
}

func TestEventNameTransliteration(t *testing.T) {
	for _, tc := range []struct {
		name, content, want string
		speaker             *string
		llmResult           string
		calls               int
	}{
		{"Latin-free name", "太郎さんが新しくフォローしてくれました。", "太郎さんが新しくフォローしてくれました。", new("太郎"), "", 0},
		{"LLM failure", "Bobさんが新しくフォローしてくれました。", "Bobさんが新しくフォローしてくれました。", new("Bob"), "", 1},
		{"unknown speaker", "誰かさんが新しくフォローしてくれました。", "誰かさんが新しくフォローしてくれました。", nil, "", 0},
		{"successful replacement", "Bobさん、Bobさんありがとう", "ボブさん、ボブさんありがとう", new("Bob"), " ボブ\n", 1},
	} {
		t.Run(tc.name, func(t *testing.T) {
			e := testEngine(config.DefaultTalkConfig())
			calls := 0
			e.llm = chatFunc(func(_ context.Context, req llm.ChatRequest) (llm.ChatResponse, error) {
				calls++
				if stringOrEmpty(req.ReasoningEffort) != "none" || len(req.Messages) != 2 ||
					req.Messages[0].Content.AsText() != transliterateNamePrompt || req.Messages[1].Content.AsText() != stringOrEmpty(tc.speaker) {
					t.Errorf("unexpected transliteration request: %+v", req)
				}
				if tc.llmResult == "" {
					return llm.ChatResponse{}, errors.New("endpoint unreachable")
				}
				return completion(tc.llmResult), nil
			})
			got := maybeTransliterateEventName(t.Context(), e, tc.content, tc.speaker)
			if got != tc.want || calls != tc.calls {
				t.Errorf("transliteration = %q in %d calls, want %q in %d calls", got, calls, tc.want, tc.calls)
			}
		})
	}
}

func TestModuleRunHandlesBusAndShutdown(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		cfg := config.DefaultConfig()
		cfg.Providers = []config.ProviderConfig{{ID: "test", BaseURL: "http://localhost:0/v1", Enabled: true}}
		cfg.Talk.ModelRef = &config.ModelRef{ProviderID: "test", Model: "test-model"}
		b := bus.New()
		mc := module.Ctx{Bus: b, Config: &cfg}
		m, err := New(mc)
		if err != nil || m.Name() != "npc-talk" {
			t.Fatalf("New = %v, %v", m, err)
		}
		ctx, cancel := context.WithCancel(t.Context())
		defer cancel()
		result := make(chan error, 1)
		go func() { result <- m.Run(ctx, mc) }()
		sub := b.Subscribe()
		defer sub.Close()
		synctest.Wait()
		select {
		case err := <-result:
			t.Fatalf("module stopped during startup: %v", err)
		default:
		}

		updated := cfg
		updated.Translation.ModeValue = "interpret"
		b.Publish(bus.TopicConfig, bus.MsgConfigUpdated, updated)
		b.Publish(bus.TopicSense, bus.MsgSpeech, bus.SpeechPayload("じゃあね", "太郎", "ignored", false))
		b.Publish(bus.TopicInterrupt, bus.MsgSuspend, nil)
		updated.Translation.ModeValue = "assist"
		b.Publish(bus.TopicConfig, bus.MsgConfigUpdated, updated)
		b.Publish(bus.TopicSense, bus.MsgSpeech, bus.SpeechPayload("じゃあね", "太郎", "suspended", false))
		synctest.Wait()
		if got := findMessages(drainMessages(sub), bus.MsgChatResponse); len(got) != 0 {
			t.Fatalf("gated turns answered: %+v", got)
		}

		time.Sleep(suspendTimeout + time.Second)
		b.Publish(bus.TopicSense, bus.MsgSpeech, bus.SpeechPayload("じゃあね", "太郎", "answered", false))
		b.Publish(bus.TopicSense, bus.MsgSpeech, bus.SpeechPayload("うん", "太郎", "silent", false))
		synctest.Wait()
		messages := drainMessages(sub)
		answered := findMessages(messages, bus.MsgChatResponse)
		silent := findMessages(messages, bus.MsgChatSilent)
		if len(answered) != 1 || answered[0].Str("request_id") != "answered" ||
			len(silent) != 1 || silent[0].Str("request_id") != "silent" {
			t.Fatalf("unexpected bus outcomes: answered=%+v, silent=%+v", answered, silent)
		}
		cancel()
		if err := <-result; err != nil {
			t.Fatal(err)
		}
	})
}
