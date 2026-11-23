package llm

import (
	"bytes"
	"encoding/json"
	"testing"
)

func TestChatRequestNeverSerializesTemperature(t *testing.T) {
	req := NewChatRequest("raw-model", nil)
	effort := "none"
	req.ReasoningEffort = &effort
	wire, err := json.Marshal(req)
	if err != nil {
		t.Fatal(err)
	}
	checkJSON(t, bytes.NewReader(wire), `{"model":"raw-model","messages":[],"reasoning_effort":"none"}`)
}

func TestChatContentRoundTrip(t *testing.T) {
	for _, tt := range []struct{ wire, text string }{
		{`"hello"`, "hello"},
		{`[]`, ""},
		{`[{"type":"text","text":"first"},{"type":"image_url","image_url":{"url":"data:image/png;base64,QQ=="}},{"type":"text","text":"second"}]`, "first\nsecond"},
	} {
		var content ChatContent
		if err := json.Unmarshal([]byte(tt.wire), &content); err != nil {
			t.Fatal(err)
		}
		if got := content.AsText(); got != tt.text {
			t.Errorf("AsText for %s = %q, want %q", tt.wire, got, tt.text)
		}
		encoded, err := json.Marshal(content)
		if err != nil {
			t.Fatal(err)
		}
		checkJSON(t, bytes.NewReader(encoded), tt.wire)
	}
}

func TestChatOptionalArrays(t *testing.T) {
	req := NewChatRequest("m", []ChatMessage{UserMessage("hello"), UserPartsMessage(nil)})
	req.Messages[0].ToolCalls = []ToolCall{}
	req.Tools = []Tool{}
	wire, err := json.Marshal(req)
	if err != nil {
		t.Fatal(err)
	}
	checkJSON(t, bytes.NewReader(wire), `{"model":"m","messages":[{"role":"user","content":"hello","tool_calls":[]},{"role":"user","content":[]}],"tools":[]}`)
}

func TestDecodeChatDefaults(t *testing.T) {
	var result ChatResponse
	if err := json.Unmarshal([]byte(`{}`), &result); err != nil {
		t.Fatal(err)
	}
	if result.ID != nil || len(result.Choices) != 0 || result.Content() != nil || len(result.ToolCalls()) != 0 {
		t.Errorf("empty response decoded to %+v", result)
	}
	if err := json.Unmarshal([]byte(`{"id":null,"choices":[{"message":{"content":""}}]}`), &result); err != nil {
		t.Fatal(err)
	}
	if result.Content() == nil || *result.Content() != "" || result.Choices[0].Message.Role != "" {
		t.Errorf("missing role and empty content decoded to %+v", result)
	}
	var tool Tool
	if err := json.Unmarshal([]byte(`{"function":{"name":"f","parameters":null}}`), &tool); err != nil {
		t.Fatal(err)
	}
	if tool.Type != "function" || tool.Function.Description != nil || string(tool.Function.Parameters) != "null" {
		t.Errorf("tool defaults decoded to %+v", tool)
	}
	var call ToolCall
	if err := json.Unmarshal([]byte(`{"function":{"name":"f","arguments":"{}"}}`), &call); err != nil {
		t.Fatal(err)
	}
	if call.ID != "" || call.Type != "function" {
		t.Errorf("tool call defaults decoded to %+v", call)
	}
}

func TestRejectInvalidChatTypes(t *testing.T) {
	for _, tt := range []struct {
		wire   string
		target any
	}{
		{`null`, &ChatContent{}},
		{`[null]`, &ChatContent{}},
		{`[{"type":"text"}]`, &ChatContent{}},
		{`[{"type":"image_url","image_url":{"URL":"x"}}]`, &ChatContent{}},
		{`[{"type":"video"}]`, &ChatContent{}},
		{`{"role":"user","content":"a","content":"b"}`, &ChatMessage{}},
		{`{"role":"user","content":null}`, &ChatMessage{}},
		{`{"type":null,"function":{"name":"f","parameters":null}}`, &Tool{}},
		{`{"function":{"name":"f"}}`, &Tool{}},
		{`{"function":{"name":"f","arguments":null}}`, &ToolCall{}},
		{`{"choices":[{"message":{"role":null}}]}`, &ChatResponse{}},
		{`{"data":[{"embedding":[null]}]}`, &embeddingResponse{}},
		{`{"data":[{}]}`, &modelResponse{}},
		{`{"text":null}`, &transcriptResponse{}},
	} {
		if err := json.Unmarshal([]byte(tt.wire), tt.target); err == nil {
			t.Errorf("invalid JSON shape %s decoded into %T", tt.wire, tt.target)
		}
	}
}
