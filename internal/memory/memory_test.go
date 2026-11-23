package memory

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/chatlog"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/module"
	"github.com/tik-choco/tc-npc/internal/person"
)

func TestModulePublishesMemoryAndFlushesPendingTurns(t *testing.T) {
	dir := t.TempDir()
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	speaker := person.NewPerson("太郎", "manual")
	if err := person.SavePerson(dir, &speaker); err != nil {
		t.Fatal(err)
	}
	storePath := filepath.Join(dir, storeFileName)
	store := loadVectorStore(storePath)
	angle := []float32{0.9, 0.43589}
	store.addChunk("old", "linked memory", angle, map[string]string{"person_id": speaker.ID}, 1)
	store.addChunk("old", "unlinked memory", angle, nil, 1)
	if err := store.save(); err != nil {
		t.Fatal(err)
	}

	cfg := config.DefaultConfig()
	cfg.Language = "en"
	cfg.Memory.Threshold = 0.93
	cfg.Memory.SummarizePrompt = "Summarize"
	cfg.Memory.ConsolidatePrompt = "Consolidate"
	cfg.Memory.ChunkSize, cfg.Memory.ChunkOverlap = 3, 1
	cfg.Memory.ReasoningEffort = "low"
	base := "\n\nAlways write your reply in English."
	turn := "User: hi\nAI: hello"
	turns := strings.TrimSuffix(strings.Repeat(turn+"\n", 5), "\n")
	chatURL := serveChat(t, "memory-chat", "chat-key", "low", map[string]string{
		"Summarize" + base + "\nPrevious summary: \nNew conversations:\n" + turns:                              "short summary",
		"Consolidate" + base + "\nPrevious summary: short summary\nNew conversations:\nUser: goodbye\nAI: bye": "あいうえお",
	})
	embed := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req struct {
			Model string   `json:"model"`
			Input []string `json:"input"`
		}
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Errorf("decode embedding request: %v", err)
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}
		if r.URL.Path != "/v1/embeddings" || r.Header.Get("Authorization") != "Bearer embed-key" || req.Model != "memory-embedding" {
			t.Errorf("unexpected embedding request %s, model %q", r.URL.Path, req.Model)
		}
		switch {
		case slices.Equal(req.Input, []string{"query"}):
			fmt.Fprint(w, `{"data":[{"embedding":[1,0]}]}`)
		case slices.Equal(req.Input, []string{"あいう", "うえお"}):
			fmt.Fprint(w, `{"data":[{"embedding":[0,1]},{"embedding":[1,0]}]}`)
		default:
			t.Errorf("unexpected embedding input %q", req.Input)
			http.Error(w, "unexpected input", http.StatusBadRequest)
		}
	}))
	t.Cleanup(embed.Close)
	cfg.Providers = []config.ProviderConfig{
		{ID: "chat", BaseURL: chatURL, APIKey: "chat-key", Enabled: true},
		{ID: "embed", BaseURL: embed.URL, APIKey: "embed-key", Enabled: true},
	}
	cfg.Memory.ModelRef = &config.ModelRef{ProviderID: "chat", Model: "memory-chat"}
	cfg.Memory.EmbeddingRef = &config.ModelRef{ProviderID: "embed", Model: "memory-embedding"}
	mc := module.Ctx{Bus: b, Config: &cfg, DataDir: dir}
	m, err := New(mc)
	if err != nil || m.Name() != "npc-memory" {
		t.Fatalf("New = %v, %v", m, err)
	}
	ctx, cancel := context.WithCancel(t.Context())
	done := make(chan struct{})
	var runErr error
	go func() {
		defer close(done)
		runErr = m.Run(ctx, mc)
	}()
	defer func() {
		cancel()
		<-done
	}()

	// A speaker-only message confirms the module has subscribed without
	// initiating an embedding request.
	profile := waitForProfile(t, b, sub)
	if profile.Str("person_id") != speaker.ID || profile.Str("name") != speaker.Name || !strings.Contains(profile.Str("content"), "太郎") || len(profile.Map()) != 3 {
		t.Fatalf("speaker profile = %s", profile.Payload)
	}
	b.Publish(bus.TopicSense, bus.MsgSpeech, bus.SpeechPayload("query", "太郎さん", "", false))
	assertMemory(t, waitForTopic(t, sub, bus.TopicMem), bus.MsgLongTermMemory, "linked memory")

	b.Publish(bus.TopicSense, bus.MsgPersonSeen, map[string]string{"name": "花子", "appearance": "青い帽子", "note": "手を振っている", "source": "vision"})
	update := waitForTopic(t, sub, bus.TopicUI)
	if update.Type != bus.MsgPersonUpdated || update.Str("name") != "花子" || update.Str("appearance") != "青い帽子" || update.Map()["encounterCount"] != float64(1) {
		t.Fatalf("vision update = %s / %s", update.Type, update.Payload)
	}
	p, err := person.LoadPerson(dir, update.Str("id"))
	if err != nil || p == nil || len(p.Facts) != 1 || p.Facts[0].Text != "手を振っている" || p.Facts[0].Source != "vision" {
		t.Fatalf("saved vision record = %+v, %v", p, err)
	}

	for range 5 {
		b.Publish(bus.TopicChat, bus.MsgChatLog, map[string]string{"input": "hi", "output": "hello"})
	}
	assertMemory(t, waitForTopic(t, sub, bus.TopicMem), bus.MsgShortTermMemory, "short summary")
	b.Publish(bus.TopicChat, bus.MsgChatLog, map[string]string{"input": "goodbye", "output": "bye", "speaker": speaker.Name})
	cancel()
	select {
	case <-done:
		if runErr != nil {
			t.Fatal(runErr)
		}
	case <-time.After(12 * time.Second):
		t.Fatal("module did not stop")
	}
	assertMemory(t, waitForTopic(t, sub, bus.TopicMem), bus.MsgLongTermMemory, "あいうえお")
	entries := chatlog.LoadRecent(dir, 10)
	if len(entries) != 6 || entries[5].Input != "goodbye" || entries[5].Speaker == nil || *entries[5].Speaker != speaker.Name {
		t.Fatalf("raw chat entries = %+v", entries)
	}
	records := ReadLongTermRecords(dir)
	if len(records) != 4 || records[0].Text != "あいう" || records[1].Text != "うえお" || records[0].DocID != records[1].DocID {
		t.Fatalf("long-term records = %+v", records)
	}
	for _, record := range records[:2] {
		if record.Metadata["person_id"] != speaker.ID || record.Metadata["type"] != bus.MsgLongTermMemory {
			t.Errorf("chunk metadata = %+v", record.Metadata)
		}
		if _, err := time.Parse(time.RFC3339, record.Metadata["timestamp"]); err != nil {
			t.Errorf("chunk timestamp: %v", err)
		}
	}
}

func TestIdleFlushExtractsFactsFromTurnsWithoutEmbeddings(t *testing.T) {
	const turn = "User: I like coffee\nAI: Me too"
	const response = "Found these facts.\n```json\n" + `[{"name":"太郎さん","facts":["Likes coffee"," Likes coffee "]},{"name":"","facts":["Ignored"]}]` + "\n```"
	chatURL := serveChat(t, "memory-chat", "chat-key", "low", map[string]string{
		"Summarize\nPrevious summary: older summary\nNew conversations:\n" + turn: "new summary",
		"Extract\n\n" + turn: response,
	})
	tracker := loadPersonTracker(t.TempDir(), 20)
	id := tracker.observeFromChat("太郎").ID
	s := memorySession{
		bus: bus.New(), tracker: tracker,
		cfg:  config.MemoryConfig{SummarizePrompt: "Summarize", People: config.PersonMemoryConfig{ExtractPrompt: "Extract"}},
		chat: llm.New(chatURL, "chat-key").WithReasoningEffort("low"), chatModel: "memory-chat",
		buffer: []string{turn}, summary: "older summary",
	}
	sub := s.bus.Subscribe()
	defer sub.Close()
	s.flushIdle(t.Context())
	assertMemory(t, waitForTopic(t, sub, bus.TopicMem), bus.MsgShortTermMemory, "new summary")
	update := waitForTopic(t, sub, bus.TopicUI)
	if update.Type != bus.MsgPersonUpdated || update.Str("id") != id || s.summary != "new summary" || len(s.buffer) != 0 {
		t.Fatalf("idle flush state: update=%s, summary=%q, buffer=%v", update.Payload, s.summary, s.buffer)
	}
	p, err := person.LoadPerson(tracker.dataDir, id)
	if err != nil || p == nil || len(p.Facts) != 1 || p.Facts[0].Text != "Likes coffee" || p.Facts[0].Source != "chat" || p.EncounterCount != 2 {
		t.Fatalf("saved extracted facts = %+v, %v", p, err)
	}
}

func serveChat(t *testing.T, model, key, effort string, replies map[string]string) string {
	t.Helper()
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var req llm.ChatRequest
		if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
			t.Errorf("decode chat request: %v", err)
			http.Error(w, "bad request", http.StatusBadRequest)
			return
		}
		if r.URL.Path != "/v1/chat/completions" || r.Header.Get("Authorization") != "Bearer "+key || req.Model != model || req.ReasoningEffort == nil || *req.ReasoningEffort != effort {
			t.Errorf("unexpected chat request %s, model %q", r.URL.Path, req.Model)
		}
		if len(req.Messages) != 1 || req.Messages[0].Role != "user" {
			t.Errorf("unexpected chat messages %+v", req.Messages)
			http.Error(w, "unexpected messages", http.StatusBadRequest)
			return
		}
		prompt := req.Messages[0].Content.AsText()
		reply, ok := replies[prompt]
		if !ok {
			t.Errorf("unexpected chat prompt %q", prompt)
			http.Error(w, "unexpected prompt", http.StatusBadRequest)
			return
		}
		fmt.Fprintf(w, `{"choices":[{"message":{"role":"assistant","content":%q}}]}`, reply)
	}))
	t.Cleanup(server.Close)
	return server.URL
}

func waitForProfile(t *testing.T, b *bus.Bus, sub *bus.Subscription) bus.Message {
	t.Helper()
	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	timeout := time.After(5 * time.Second)
	for {
		select {
		case <-ticker.C:
			b.Publish(bus.TopicSense, bus.MsgSpeech, map[string]string{"speaker": "太郎"})
		case msg := <-sub.C:
			if msg.Topic == bus.TopicMem && msg.Type == bus.MsgPersonMemory {
				return msg
			}
		case <-timeout:
			t.Fatal("timed out waiting for speaker profile")
		}
	}
}

func waitForTopic(t *testing.T, sub *bus.Subscription, topic string) bus.Message {
	t.Helper()
	timeout := time.After(5 * time.Second)
	for {
		select {
		case msg := <-sub.C:
			if msg.Topic == topic {
				return msg
			}
		case <-timeout:
			t.Fatalf("timed out waiting for %s", topic)
		}
	}
}

func assertMemory(t *testing.T, msg bus.Message, typ, content string) {
	t.Helper()
	if msg.Type != typ || !reflect.DeepEqual(msg.Map(), map[string]any{"content": content}) {
		t.Fatalf("memory message = %s / %s, want %s / %q", msg.Type, msg.Payload, typ, content)
	}
}
