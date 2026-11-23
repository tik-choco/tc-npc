package vision

import (
	"context"
	"encoding/json"
	"fmt"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/llm"
)

func toolCall(name, args string) llm.ToolCall {
	return llm.ToolCall{
		ID: "call-1", Type: "function",
		Function: llm.FunctionCall{Name: name, Arguments: args},
	}
}

func nextMessage(t *testing.T, sub *bus.Subscription) bus.Message {
	t.Helper()
	select {
	case msg := <-sub.C:
		return msg
	default:
		t.Fatal("expected a published message")
		return bus.Message{}
	}
}

func TestBuildTools(t *testing.T) {
	for _, tc := range []struct {
		name    string
		enabled bool
		want    []string
	}{
		{"person detection off", false, []string{robotTool}},
		{"person detection on", true, []string{robotTool, personTool}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			var names []string
			for _, tool := range buildTools(tc.enabled) {
				names = append(names, tool.Function.Name)
			}
			if !reflect.DeepEqual(names, tc.want) {
				t.Fatalf("tool names = %v, want %v", names, tc.want)
			}
		})
	}
}

func TestExecuteObservePerson(t *testing.T) {
	for _, tc := range []struct {
		name, args, person, appearance, note string
	}{
		{
			name: "publishes person seen", args: `{"name":"太郎","appearance":"blue hoodie, short black hair","note":"waving"}`,
			person: "太郎", appearance: "blue hoodie, short black hair", note: "waving",
		},
		{name: "defaults name and note", args: `{"appearance":"red cap"}`, appearance: "red cap"},
		{name: "accepts positional arguments", args: `["太郎","red cap"]`, person: "太郎", appearance: "red cap"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			b := bus.New()
			sub := b.Subscribe()
			defer sub.Close()

			result := executeToolCall(toolCall(personTool, tc.args), b)
			if !strings.Contains(result, tc.appearance) {
				t.Errorf("tool result %q lacks appearance %q", result, tc.appearance)
			}
			got := nextMessage(t, sub)
			if got.Topic != bus.TopicSense || got.Type != bus.MsgPersonSeen {
				t.Errorf("unexpected topic or type: %s / %s", got.Topic, got.Type)
			}
			want := map[string]any{"name": tc.person, "appearance": tc.appearance, "note": tc.note, "source": "vision"}
			if !reflect.DeepEqual(got.Map(), want) {
				t.Errorf("payload = %v, want %v", got.Map(), want)
			}
		})
	}
}

func TestExecuteRobotCommand(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()

	if result := executeToolCall(toolCall(robotTool, `{"command":"jump"}`), b); result != "Successfully sent command: jump" {
		t.Errorf("unexpected tool result: %q", result)
	}
	got := nextMessage(t, sub)
	if got.Topic != bus.TopicAction || got.Type != bus.MsgAction || string(got.Payload) != `"jump"` {
		t.Errorf("unexpected action: %+v", got)
	}
}

func TestUnknownToolName(t *testing.T) {
	got := executeToolCall(toolCall("bogus_tool", `{}`), bus.New())
	if got != "Error executing tool: unknown tool 'bogus_tool'" {
		t.Errorf("unexpected tool result: %q", got)
	}
}

func TestInvalidPersonArgumentsDoNotPublish(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()

	for _, args := range []string{
		`{}`, `{"Appearance":"hat"}`, `{"appearance":null}`, `{"appearance":123}`,
		`{"appearance":"hat","note":null}`, `{"appearance":"hat","appearance":"cap"}`,
	} {
		result := executeToolCall(toolCall(personTool, args), b)
		if !strings.HasPrefix(result, "Error executing tool: ") {
			t.Errorf("arguments %s returned %q", args, result)
		}
	}
	select {
	case got := <-sub.C:
		t.Errorf("invalid arguments published %+v", got)
	default:
	}
}

func TestAnalyzeImageToolFollowUp(t *testing.T) {
	initial := `{"choices":[{"message":{"content":"initial","tool_calls":[{"id":"person-1","function":{"name":"observe_person","arguments":"{\"appearance\":\"red cap\"}"}}]}}]}`
	for _, tc := range []struct {
		name, reply, want string
	}{
		{"final narrative", `{"choices":[{"message":{"content":"A person is waving."}}]}`, "A person is waving."},
		{"fallback narrative", `{"choices":[{"message":{"content":""}}]}`, "initial\n\n【アクション実行結果】\nRecorded observation of person: red cap"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			requests := make(chan llm.ChatRequest, 2)
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				var req llm.ChatRequest
				if err := json.NewDecoder(r.Body).Decode(&req); err != nil {
					t.Errorf("read request: %v", err)
					w.WriteHeader(http.StatusBadRequest)
					return
				}
				select {
				case requests <- req:
				default:
					t.Error("unexpected third completion")
					w.WriteHeader(http.StatusBadRequest)
					return
				}
				if len(req.Messages) == 2 {
					fmt.Fprint(w, initial)
				} else {
					fmt.Fprint(w, tc.reply)
				}
			}))
			defer server.Close()

			b := bus.New()
			sub := b.Subscribe()
			defer sub.Close()
			imageURL := "data:image/png;base64,abcd"
			client := llm.New(server.URL, "")
			got, err := analyzeImage(context.Background(), client, "test-model", "system", imageURL, "recent speech", b, true)
			if err != nil || got != tc.want {
				t.Fatalf("analysis = %q, %v, want %q", got, err, tc.want)
			}
			if len(requests) != 2 {
				t.Fatalf("got %d requests, want two", len(requests))
			}
			first, second := <-requests, <-requests
			if first.Model != "test-model" || first.MaxTokens == nil || *first.MaxTokens != 500 || len(first.Tools) != 2 {
				t.Fatalf("unexpected initial request: %+v", first)
			}
			parts := []llm.ContentPart{
				llm.ImageDataURLPart(imageURL),
				llm.TextPart("【短期記憶】: recent speech\n\n現在の視界を分析し、必要に応じて行動を選択してください。"),
			}
			if !reflect.DeepEqual(first.Messages, []llm.ChatMessage{llm.SystemMessage("system"), llm.UserPartsMessage(parts)}) {
				t.Errorf("unexpected initial messages: %+v", first.Messages)
			}
			if second.Tools != nil || second.MaxTokens == nil || *second.MaxTokens != 500 || len(second.Messages) != 4 {
				t.Fatalf("unexpected follow-up request: %+v", second)
			}
			assistant := second.Messages[2]
			if assistant.Role != "assistant" || assistant.Content.Text != "initial" || len(assistant.ToolCalls) != 1 || assistant.ToolCalls[0].ID != "person-1" {
				t.Errorf("unexpected assistant message: %+v", assistant)
			}
			wantResult := llm.ToolResultMessage("person-1", "Recorded observation of person: red cap")
			if !reflect.DeepEqual(second.Messages[3], wantResult) {
				t.Errorf("unexpected tool result message: %+v", second.Messages[3])
			}
			if msg := nextMessage(t, sub); msg.Type != bus.MsgPersonSeen || msg.Str("appearance") != "red cap" {
				t.Errorf("unexpected observation: %+v", msg)
			}
		})
	}
}
