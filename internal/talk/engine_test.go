package talk

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"maps"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
)

type chatFunc func(context.Context, llm.ChatRequest) (llm.ChatResponse, error)

func (f chatFunc) Chat(ctx context.Context, req llm.ChatRequest) (llm.ChatResponse, error) {
	return f(ctx, req)
}

func testEngine(cfg config.TalkConfig) *ChatEngine {
	e := NewChatEngine(nil, "test-model", cfg, ToolRegistry{}, nil, "auto", bus.New())
	e.llm = chatFunc(func(context.Context, llm.ChatRequest) (llm.ChatResponse, error) {
		return llm.ChatResponse{}, errors.New("endpoint unreachable")
	})
	return e
}

func completion(content string) llm.ChatResponse {
	return llm.ChatResponse{Choices: []llm.ChatChoice{{Message: llm.ResponseMessage{Content: &content}}}}
}

func TestFillTemplate(t *testing.T) {
	vars := map[string]string{"session_meta": "2026-07-24 12:00:00 (Fri)", "short_term_memory": "hello", "persona": "P"}
	for _, tc := range []struct{ input, want string }{
		{"Now: {{session_meta}}. Memory: {{short_term_memory}}.", "Now: 2026-07-24 12:00:00 (Fri). Memory: hello."},
		{"Persona: {{absent}} end", "Persona:  end"},
		{"  {{missing}}  ", ""},
		{"literal {not a placeholder} {{persona}}", "literal {not a placeholder} P"},
	} {
		if got := fillTemplate(tc.input, vars); got != tc.want {
			t.Errorf("fillTemplate(%q) = %q, want %q", tc.input, got, tc.want)
		}
	}
}

func TestTrimHistory(t *testing.T) {
	pairs := func(count int) []llm.ChatMessage {
		var h []llm.ChatMessage
		for i := range count {
			h = append(h, llm.UserMessage(fmt.Sprintf("u%d", i)), llm.AssistantMessage(fmt.Sprintf("a%d", i)))
		}
		return h
	}
	longAssistantTail := []llm.ChatMessage{llm.UserMessage("u0")}
	for i := range 10 {
		longAssistantTail = append(longAssistantTail, llm.AssistantMessage(fmt.Sprintf("a%d", i)))
	}
	for _, tc := range []struct {
		name, first, last string
		role              string
		history           []llm.ChatMessage
		size, count       int
	}{
		{"within limit", "u0", "a0", "user", pairs(1), 20, 2},
		{"user boundary", "u3", "a4", "user", pairs(5), 4, 4},
		{"hard cut", "a7", "a9", "assistant", longAssistantTail, 3, 3},
		{"zero size", "", "", "", pairs(1), 0, 0},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got := trimHistory(tc.history, tc.size)
			if len(got) != tc.count {
				t.Fatalf("history length = %d, want %d", len(got), tc.count)
			}
			if tc.count > 0 && (got[0].Role != tc.role || got[0].Content.AsText() != tc.first ||
				got[len(got)-1].Content.AsText() != tc.last) {
				t.Errorf("unexpected history: %+v", got)
			}
		})
	}
}

func TestChatPayloads(t *testing.T) {
	for _, tc := range []struct {
		name    string
		payload any
		want    map[string]any
	}{
		{"log with speaker", chatLogPayload("hi", "hello", new("太郎")), map[string]any{"input": "hi", "output": "hello", "speaker": "太郎"}},
		{"log without speaker", chatLogPayload("hi", "hello", nil), map[string]any{"input": "hi", "output": "hello"}},
		{"response with requester", chatResponsePayload("hello", "hi", new("req-1")), map[string]any{"content": "hello", "input": "hi", "request_id": "req-1"}},
		{"response without requester", chatResponsePayload("hello", "hi", nil), map[string]any{"content": "hello", "input": "hi"}},
		{"response with empty requester", chatResponsePayload("hello", "hi", new("")), map[string]any{"content": "hello", "input": "hi", "request_id": ""}},
		{"silent with requester", chatSilentPayload("hi", "declined", new("req-2")), map[string]any{"reason": "declined", "input": "hi", "request_id": "req-2"}},
		{"silent without requester", chatSilentPayload("hi", "declined", nil), map[string]any{"reason": "declined", "input": "hi"}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			raw, err := json.Marshal(tc.payload)
			if err != nil {
				t.Fatal(err)
			}
			got := (bus.Message{Payload: raw}).Map()
			if strings.HasPrefix(tc.name, "log ") {
				stamp, _ := got["time"].(string)
				if _, err := time.Parse(time.RFC3339Nano, stamp); err != nil || !strings.HasSuffix(stamp, "+00:00") {
					t.Errorf("chat log time = %q, want Rust RFC3339 time", stamp)
				}
				delete(got, "time")
			}
			if !maps.Equal(got, tc.want) {
				t.Errorf("payload = %v, want %v", got, tc.want)
			}
		})
	}
}

func drainMessages(sub *bus.Subscription) []bus.Message {
	var messages []bus.Message
	for {
		select {
		case message := <-sub.C:
			messages = append(messages, message)
		default:
			return messages
		}
	}
}

func findMessages(messages []bus.Message, kind string) []bus.Message {
	var found []bus.Message
	for _, msg := range messages {
		if msg.Type == kind {
			found = append(found, msg)
		}
	}
	return found
}

func TestClosingTurnSpeechSilenceAndCorrelation(t *testing.T) {
	for _, tc := range []struct {
		name         string
		allowSilence bool
		firstID      *string
		secondID     *string
		secondReply  string
		outcome      string
	}{
		{"request IDs survive both outcomes", true, new("req-1"), new("req-2"), "", bus.MsgChatSilent},
		{"no IDs for self-initiated turns", true, nil, nil, "", bus.MsgChatSilent},
		{"silence disabled", false, nil, nil, "……", bus.MsgChatResponse},
	} {
		t.Run(tc.name, func(t *testing.T) {
			cfg := config.DefaultTalkConfig()
			cfg.AllowSilence = tc.allowSilence
			e := testEngine(cfg)
			sub := e.bus.Subscribe()
			defer sub.Close()

			first, err := e.ChatWithContext(t.Context(), "じゃあね", nil, tc.firstID)
			if err != nil || first != "またね。" {
				t.Fatalf("farewell = %q, %v", first, err)
			}
			second, err := e.ChatWithContext(t.Context(), "うん", nil, tc.secondID)
			if err != nil || second != tc.secondReply {
				t.Fatalf("acknowledgment = %q, %v, want %q", second, err, tc.secondReply)
			}
			messages := drainMessages(sub)
			wantTypes := []string{bus.MsgAffectState, bus.MsgChatResponse, bus.MsgChatLog, bus.MsgAffectState, tc.outcome, bus.MsgChatLog}
			var types []string
			for _, msg := range messages {
				types = append(types, msg.Type)
			}
			if !slices.Equal(types, wantTypes) {
				t.Fatalf("messages = %v, want %v", types, wantTypes)
			}
			for i, id := range []*string{tc.firstID, tc.secondID} {
				payload := messages[1+3*i].Map()
				value, found := payload["request_id"]
				if id == nil && found || id != nil && value != *id {
					t.Errorf("turn %d: request_id = %v, present %v, want %v", i, value, found, id)
				}
			}
			if tc.allowSilence && (messages[4].Str("reason") != "closing" ||
				messages[5].Str("output") != "" || len(e.state.history) != 3 || e.state.history[2].Role != "user") {
				t.Errorf("silent turn published or stored an assistant reply: %v, %v", messages[4].Map(), messages[5].Map())
			}
		})
	}
}

func TestStyleRulesEnabledByDefault(t *testing.T) {
	cfg := config.DefaultTalkConfig()
	e := testEngine(cfg)
	if message := e.styleMessage(); message == nil || message.Role != "system" || !strings.Contains(message.Content.AsText(), "ト書き") {
		t.Fatalf("default style message = %+v", message)
	}
	cfg.StyleRules = false
	if testEngine(cfg).styleMessage() != nil {
		t.Fatal("disabled style rules were injected")
	}
}

func TestPersonMemoryTemplateVariable(t *testing.T) {
	vars := buildVars("", "", "太郎について: 犬好き")
	if vars["person_memory"] != "太郎について: 犬好き" {
		t.Errorf("person memory = %q", vars["person_memory"])
	}
	if _, ok := buildVars("", "", "")["person_memory"]; ok {
		t.Error("empty person memory added to template variables")
	}
	vars = buildVars("", "", "太郎: 犬好き")
	if got := fillTemplate("Person: {{person_memory}}", vars); got != "Person: 太郎: 犬好き" {
		t.Errorf("person memory expansion = %q", got)
	}
}

func TestFailedTurnOnlyPublishesErrorForRequester(t *testing.T) {
	for _, id := range []*string{new("req-7"), nil} {
		e := testEngine(config.DefaultTalkConfig())
		sub := e.bus.Subscribe()
		defer sub.Close()
		if _, err := e.ChatWithContext(t.Context(), "こんにちは", nil, id); err == nil {
			t.Fatal("expected a failed LLM call")
		}
		messages := drainMessages(sub)
		failed := findMessages(messages, bus.MsgChatError)
		if id == nil {
			if len(failed) != 0 {
				t.Fatalf("self-initiated error was published: %v", failed)
			}
		} else if len(failed) != 1 || failed[0].Str("request_id") != *id || failed[0].Str("message") == "" {
			t.Fatalf("chat error = %v, want one matching requester and an error message", failed)
		}
		if len(findMessages(messages, bus.MsgChatResponse)) != 0 || len(findMessages(messages, bus.MsgChatSilent)) != 0 {
			t.Fatalf("failed turn published an additional outcome: %v", messages)
		}
	}
}

func TestHasLatinLetters(t *testing.T) {
	for name, want := range map[string]bool{"Bob": true, "たろうBob": true, "太郎": false, "たろう123": false, "": false} {
		if got := hasLatinLetters(name); got != want {
			t.Errorf("hasLatinLetters(%q) = %v, want %v", name, got, want)
		}
	}
}

func TestTransliterateNameFallsBackOnFailure(t *testing.T) {
	e := testEngine(config.DefaultTalkConfig())
	if got := e.TransliterateName(t.Context(), "Bob"); got != "Bob" {
		t.Errorf("failed transliteration returned %q", got)
	}
}

// The port must preserve prompt order and retain the whole tool exchange
// in history while sending the filter its own two-message request.
func TestEnginePromptToolAndFilterOrder(t *testing.T) {
	cfg := config.DefaultTalkConfig()
	cfg.FilterPromptName = "filter"
	cfg.Prompts = []config.PromptEntry{
		{Name: "core", Content: "{{persona}} | {{short_term_memory}} | {{long_term_memory}} | {{person_memory}}"},
		{Name: "date", Content: "{{session_meta}}"},
		{Name: "skip", Content: "{{missing}}"},
		{Name: "filter", Content: "Filter this"},
	}
	var registry ToolRegistry
	registry.Register("lookup", lookupTool{})
	e := NewChatEngine(nil, "test-model", cfg, registry, new("Persona"), "ja", bus.New())
	e.SetShortTermMemory("short")
	e.SetLongTermMemory("long")
	e.SetPersonMemory("person")
	e.state.history = []llm.ChatMessage{llm.UserMessage("earlier"), llm.AssistantMessage("previous reply")}

	var calls []llm.ChatRequest
	e.llm = chatFunc(func(_ context.Context, req llm.ChatRequest) (llm.ChatResponse, error) {
		if e.mu.TryLock() {
			e.mu.Unlock()
			t.Error("turn released its state lock during an LLM call")
		}
		req.Messages = slices.Clone(req.Messages)
		calls = append(calls, req)
		switch len(calls) {
		case 1:
			return llm.ChatResponse{Choices: []llm.ChatChoice{{Message: llm.ResponseMessage{
				ToolCalls: []llm.ToolCall{{ID: "call-1", Type: "function", Function: llm.FunctionCall{Name: "lookup", Arguments: `{"q":"value"}`}}},
			}}}}, nil
		case 2:
			return completion("raw reply"), nil
		case 3:
			return completion("（微笑む） **はい。**"), nil
		default:
			return llm.ChatResponse{}, errors.New("unexpected extra LLM call")
		}
	})
	reply, err := e.Chat(t.Context(), "こんにちは")
	if err != nil || reply != "はい。" || len(calls) != 3 {
		t.Fatalf("reply = %q, %v, LLM calls = %d", reply, err, len(calls))
	}
	instruction, ok := config.LanguageInstruction("ja")
	if !ok {
		t.Fatal("missing language instruction")
	}
	first := calls[0].Messages
	if len(first) != 9 || first[0].Content.AsText() != "Persona | short | long | person" ||
		!regexp.MustCompile(`^\d{4}-\d\d-\d\d \d\d:\d\d:\d\d \([A-Z][a-z]{2}\)$`).MatchString(first[1].Content.AsText()) ||
		first[2].Content.AsText() != "Filter this" || first[3].Content.AsText() != SpokenReplyRules ||
		first[4].Content.AsText() != e.state.affect.state().ToPrompt() || first[5].Content.AsText() != instruction ||
		first[6].Content.AsText() != "earlier" || first[7].Content.AsText() != "previous reply" ||
		first[8].Role != "user" || first[8].Content.AsText() != "こんにちは" {
		t.Fatalf("unexpected prompt order: %+v", first)
	}
	if len(calls[0].Tools) != 1 || len(calls[1].Tools) != 1 || calls[2].Tools != nil {
		t.Fatalf("unexpected tool definitions across passes: %v, %v, %v", calls[0].Tools, calls[1].Tools, calls[2].Tools)
	}
	second := calls[1].Messages
	if len(second) != len(first)+2 || len(second[len(first)].ToolCalls) != 1 {
		t.Fatalf("tool call missing from followup: %+v", second)
	}
	tool := second[len(second)-1]
	if tool.Role != "tool" || stringOrEmpty(tool.ToolCallID) != "call-1" || tool.Content.AsText() != `got {"q":"value"}` {
		t.Fatalf("tool result = %+v", tool)
	}
	filter := calls[2].Messages
	if len(filter) != 2 || filter[0].Role != "system" || filter[0].Content.AsText() != "Filter this\n\n"+instruction ||
		filter[1].Role != "user" || filter[1].Content.AsText() != "raw reply" {
		t.Fatalf("unexpected filter request: %+v", filter)
	}
	history := e.state.history
	if len(history) != 6 || history[4].Content.AsText() != tool.Content.AsText() || history[5].Content.AsText() != reply {
		t.Fatalf("unexpected history: %+v", history)
	}
}

type lookupTool struct{}

func (lookupTool) Definition() llm.Tool {
	return llm.FunctionTool("lookup", "look up a value", json.RawMessage(`{"type":"object"}`))
}

func (lookupTool) Execute(_ context.Context, args string) (string, error) {
	return "got " + args, nil
}
