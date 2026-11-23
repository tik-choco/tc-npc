package translate

import (
	"bytes"
	"context"
	"encoding/json"
	"log/slog"
	"net"
	"net/http"
	"net/http/httptest"
	"testing"
	"testing/synctest"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/module"
	"github.com/tik-choco/tc-npc/internal/osc"
)

func TestExtractContent(t *testing.T) {
	for _, tc := range []struct {
		payload string
		want    string
		ok      bool
	}{
		{`"hi"`, "hi", true},
		{`{"content":"hi"}`, "hi", true},
		{`{"other":"hi"}`, "", false},
		{`{"content":null}`, "", false},
		{`null`, "", false},
	} {
		msg := bus.Message{Payload: json.RawMessage(tc.payload)}
		if got, ok := extractContent(msg); got != tc.want || ok != tc.ok {
			t.Errorf("extractContent(%s) = %q, %t, want %q, %t", tc.payload, got, ok, tc.want, tc.ok)
		}
	}
}

func TestModeScopeMatrix(t *testing.T) {
	for _, tc := range []struct {
		mode, scope  string
		input, reply bool
	}{
		{"off", "input", false, false},
		{"off", "output", false, false},
		{"off", "both", false, false},
		{"interpret", "input", true, false},
		{"interpret", "output", false, false},
		{"interpret", "both", true, false},
		{"assist", "input", true, false},
		{"assist", "output", false, true},
		{"assist", "both", true, true},
	} {
		t.Run(tc.mode+"/"+tc.scope, func(t *testing.T) {
			synctest.Test(t, func(t *testing.T) {
				settings := testSettings()
				settings.ModeValue, settings.ScopeValue = tc.mode, tc.scope
				// History records accepted events even with no target or LLM.
				settings.TargetLanguage = ""
				engine := &translationEngine{settings: settings}
				handleBusMessage(t.Context(), engine, bus.Message{
					Topic: bus.TopicSense, Type: bus.MsgSpeech, Payload: json.RawMessage(`"speech"`),
				})
				handleBusMessage(t.Context(), engine, bus.Message{
					Topic: bus.TopicChat, Type: bus.MsgChatResponse, Payload: json.RawMessage(`{"content":"reply"}`),
				})
				synctest.Wait()
				input := len(engine.userHistory.lines) > 0
				reply := len(engine.agentHistory.lines) > 0
				if input != tc.input || reply != tc.reply {
					t.Errorf("accepted input=%t reply=%t, want input=%t reply=%t", input, reply, tc.input, tc.reply)
				}
			})
		})
	}
}

func TestRunReloadsSettingsAndMirrorsResults(t *testing.T) {
	ctx, cancel := context.WithTimeout(t.Context(), 5*time.Second)
	defer cancel()

	socket, err := net.ListenUDP("udp4", &net.UDPAddr{IP: net.IPv4(127, 0, 0, 1)})
	if err != nil {
		t.Fatal(err)
	}
	defer socket.Close()
	deadline, _ := ctx.Deadline()
	if err := socket.SetReadDeadline(deadline); err != nil {
		t.Fatal(err)
	}

	type pendingRequest struct {
		request llm.ChatRequest
		reply   chan string
	}
	requests := make(chan pendingRequest, 2)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/v1/chat/completions" || r.Header.Get("Authorization") != "Bearer DUMMY-startup-key" {
			t.Error("unexpected chat path or authorization")
			http.Error(w, "unexpected request", http.StatusBadRequest)
			return
		}
		call := pendingRequest{reply: make(chan string, 1)}
		if err := json.NewDecoder(r.Body).Decode(&call.request); err != nil {
			t.Errorf("decode chat request: %v", err)
			http.Error(w, "invalid request", http.StatusBadRequest)
			return
		}
		select {
		case requests <- call:
		case <-ctx.Done():
			return
		}
		select {
		case reply := <-call.reply:
			_ = json.NewEncoder(w).Encode(llm.ChatResponse{Choices: []llm.ChatChoice{{Message: llm.ResponseMessage{Content: &reply}}}})
		case <-r.Context().Done():
		case <-ctx.Done():
		}
	}))
	t.Cleanup(server.Close)

	cfg := config.DefaultConfig()
	cfg.Language = "en"
	cfg.VRC.OSCAddress = socket.LocalAddr().String()
	cfg.Translation.ReasoningEffort = "low"
	cfg.Providers = []config.ProviderConfig{{ID: "http", BaseURL: server.URL, APIKey: "DUMMY-startup-key", Enabled: true}}
	cfg.DefaultRef = &config.ModelRef{ProviderID: "http", Model: "startup-model"}
	mc := module.Ctx{Bus: bus.New(), Config: &cfg}
	mod, err := New(mc)
	if err != nil {
		t.Fatal(err)
	}
	if mod.Name() != "npc-translate" {
		t.Fatalf("module name = %q", mod.Name())
	}
	sub := mc.Bus.Subscribe()
	defer sub.Close()
	started := make(chan struct{}, 1)
	previousLogger := slog.Default()
	slog.SetDefault(slog.New(startupLogHandler{Handler: slog.DiscardHandler, started: started}))
	t.Cleanup(func() { slog.SetDefault(previousLogger) })
	done := make(chan error, 1)
	go func() { done <- mod.Run(ctx, mc) }()
	t.Cleanup(func() {
		cancel()
		select {
		case err := <-done:
			if err != nil {
				t.Errorf("Run() returned %v", err)
			}
		case <-time.After(5 * time.Second):
			t.Error("Run() did not stop on cancellation")
		}
	})
	select {
	case <-started:
	case <-ctx.Done():
		t.Fatal("module did not subscribe while off")
	}

	mc.Bus.Publish(bus.TopicSense, bus.MsgSpeech, "ignored while off")
	updated := cfg
	updated.Providers = []config.ProviderConfig{{ID: "http", BaseURL: "http://unused.invalid", Enabled: true}}
	updated.VRC.OSCAddress = "unused.invalid:9000"
	updated.Translation.ModeValue = "interpret"
	updated.Translation.TargetLanguage2 = "中国語"
	updated.Translation.Chatbox = true
	updated.Translation.ModelRef = &config.ModelRef{ProviderID: "http", Model: "live-model"}
	updated.Translation.ReasoningEffort = "high"
	mc.Bus.Publish(bus.TopicConfig, bus.MsgConfigUpdated, updated)
	mc.Bus.Publish(bus.TopicChat, bus.MsgChatResponse, "ignored while interpreting")
	const original = "  こんにちは  "
	mc.Bus.Publish(bus.TopicSense, bus.MsgSpeech, map[string]string{"content": original})

	checkUpdate := func(want translationUpdate) {
		t.Helper()
		for {
			select {
			case msg := <-sub.C:
				if msg.Topic != bus.TopicUI || msg.Type != "translation" {
					continue
				}
				var got translationUpdate
				if err := msg.Decode(&got); err != nil || got.TS <= 0 || len(msg.Map()) != 7 {
					t.Fatalf("invalid translation payload %s (%v)", msg.Payload, err)
				}
				want.TS = got.TS
				if got != want {
					t.Fatalf("update = %+v, want %+v", got, want)
				}
				return
			case <-ctx.Done():
				t.Fatalf("timed out waiting for update %+v", want)
			}
		}
	}
	checkPacket := func(text string) {
		t.Helper()
		want, err := osc.EncodeMessage("/chatbox/input", text, true, false)
		if err != nil {
			t.Fatal(err)
		}
		var packet [1024]byte
		n, _, err := socket.ReadFromUDP(packet[:])
		if err != nil || !bytes.Equal(packet[:n], want) {
			t.Fatalf("chatbox packet = %q (%v), want %q", packet[:n], err, want)
		}
	}

	entry := translationUpdate{ID: "t1", Source: sourceUser, Original: original}
	checkUpdate(entry)
	// Both requests must start before either receives its response.
	replies := map[string]chan string{}
	for range 2 {
		select {
		case call := <-requests:
			req := call.request
			if req.Model != "live-model" || req.ReasoningEffort == nil || *req.ReasoningEffort != "low" ||
				len(req.Messages) != 1 || req.Messages[0].Role != "user" {
				t.Fatalf("unexpected chat request: %+v", req)
			}
			for _, lang := range []string{"英語", "中国語"} {
				if req.Messages[0].Content.AsText() == buildPrompt(lang, "", original) {
					replies[lang] = call.reply
				}
			}
		case <-ctx.Done():
			t.Fatal("timed out waiting for both chat requests")
		}
	}
	if len(replies) != 2 {
		t.Fatal("expected a distinct prompt for each target")
	}
	replies["中国語"] <- "  你好\n"
	entry.Lang, entry.Text = "中国語", "你好"
	checkUpdate(entry)
	checkPacket("こんにちは\n你好")
	replies["英語"] <- "Hello"
	entry.Lang, entry.Text = "英語", "Hello"
	checkUpdate(entry)
	checkPacket("こんにちは\nHello\n你好")
}

type startupLogHandler struct {
	slog.Handler
	started chan struct{}
}

func (h startupLogHandler) Enabled(_ context.Context, level slog.Level) bool {
	return level >= slog.LevelInfo
}

func (h startupLogHandler) Handle(_ context.Context, r slog.Record) error {
	if r.Message == "npc-translate: mode=off, idling until config update" {
		select {
		case h.started <- struct{}{}:
		default:
		}
	}
	return nil
}
