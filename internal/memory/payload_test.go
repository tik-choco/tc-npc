package memory

import (
	"encoding/json"
	"reflect"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/chatlog"
)

func payload(raw string) bus.Message {
	return bus.Message{Payload: json.RawMessage(raw)}
}

func TestParseChatLog(t *testing.T) {
	for _, tc := range []struct{ name, raw, want string }{
		{"formats turn", `{"input":"hi","output":"hello there"}`, "User: hi\nAI: hello there"},
		{"rejects empty", `{}`, ""},
		{"ignores transcript fields", `{"input":"hi","speaker":123}`, "User: hi\nAI: "},
		{"rejects null string", `{"input":"hi","output":null}`, ""},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := parseChatLog(payload(tc.raw)); got != tc.want {
				t.Errorf("parseChatLog = %q, want %q", got, tc.want)
			}
		})
	}
}

// These mirror the payload published by npc-talk: input, output, time, and
// an optional speaker.
func TestBuildChatLogEntry(t *testing.T) {
	const timestamp = "2026-07-26T00:00:00+00:00"
	speaker := "太郎"
	for _, tc := range []struct {
		name, raw string
		want      *chatlog.ChatLogEntry
	}{
		{"with speaker", `{"input":"hi","output":"hello there","time":"` + timestamp + `","speaker":"太郎"}`, &chatlog.ChatLogEntry{
			Input: "hi", Output: "hello there", Time: timestamp, Speaker: &speaker,
		}},
		{"speaker absent", `{"input":"hi","output":"hello there","time":"` + timestamp + `"}`, &chatlog.ChatLogEntry{
			Input: "hi", Output: "hello there", Time: timestamp,
		}},
		{"time defaults to now", `{"input":"hi","output":"hello there"}`, &chatlog.ChatLogEntry{
			Input: "hi", Output: "hello there",
		}},
		{"no input or output", `{"time":"` + timestamp + `"}`, nil},
		{"invalid speaker", `{"input":"hi","speaker":123}`, nil},
	} {
		t.Run(tc.name, func(t *testing.T) {
			got := buildChatLogEntry(payload(tc.raw))
			if tc.want != nil && tc.want.Time == "" {
				if got == nil {
					t.Fatal("buildChatLogEntry returned nil")
				}
				if _, err := time.Parse(time.RFC3339, got.Time); err != nil {
					t.Fatalf("invalid fallback timestamp %q: %v", got.Time, err)
				}
				tc.want.Time = got.Time
			}
			if !reflect.DeepEqual(got, tc.want) {
				t.Errorf("buildChatLogEntry = %+v, want %+v", got, tc.want)
			}
		})
	}
}

func TestParseSenseContent(t *testing.T) {
	for _, tc := range []struct{ raw, want string }{
		{`"hello world"`, "hello world"},
		{`{"content":"a screenshot description"}`, "a screenshot description"},
		{`""`, ""},
		{`{"content":""}`, ""},
		{`{}`, ""},
	} {
		if got := parseSenseContent(payload(tc.raw)); got != tc.want {
			t.Errorf("parseSenseContent(%s) = %q, want %q", tc.raw, got, tc.want)
		}
	}
}

func TestBuildPromptIncludesSummaryAndTurns(t *testing.T) {
	got := buildPrompt("Summarize:", "prev", []string{"turn1", "turn2"})
	want := "Summarize:\nPrevious summary: prev\nNew conversations:\nturn1\nturn2"
	if got != want {
		t.Errorf("buildPrompt = %q, want %q", got, want)
	}
}

func TestParseSpeaker(t *testing.T) {
	for _, tc := range []struct{ raw, want string }{
		{`{"speaker":" 太郎 "}`, "太郎"},
		{`{"speaker":""}`, ""},
		{`{"content":"hi"}`, ""},
		{`{"speaker":123}`, ""},
	} {
		if got := parseSpeaker(payload(tc.raw)); got != tc.want {
			t.Errorf("parseSpeaker(%s) = %q, want %q", tc.raw, got, tc.want)
		}
	}
}

func TestParsePersonSeenDefaultsMissingFields(t *testing.T) {
	if got := parsePersonSeen(payload(`{"name":"太郎","appearance":"青いパーカー"}`)); got == nil || *got != (personSeenPayload{Name: "太郎", Appearance: "青いパーカー"}) {
		t.Errorf("parsePersonSeen = %+v", got)
	}
	if got := parsePersonSeen(payload(`{}`)); got == nil || *got != (personSeenPayload{}) {
		t.Errorf("parsePersonSeen empty object = %+v", got)
	}
}

func TestParsePersonFactsResponse(t *testing.T) {
	for _, tc := range []struct {
		name, response string
		want           []extractedPersonFacts
	}{
		{"bare JSON", `[{"name":"太郎","facts":["コーヒーが好き"]}]`, []extractedPersonFacts{{Name: "太郎", Facts: []string{"コーヒーが好き"}}}},
		{"code fence and prose", "以下が抽出結果です。\n```json\n[{\"name\": \"花子\", \"facts\": [\"犬を飼っている\", \"猫アレルギー\"]}]\n```\nご確認ください。", []extractedPersonFacts{{Name: "花子", Facts: []string{"犬を飼っている", "猫アレルギー"}}}},
		{"empty array", "特に新しい情報はありません: []", []extractedPersonFacts{}},
		{"brackets in strings", `prefix [{"name":"太郎","facts":["リスト[1]の項目"]}] suffix`, []extractedPersonFacts{{Name: "太郎", Facts: []string{"リスト[1]の項目"}}}},
		{"no array", "no json here", nil},
		{"empty response", "", nil},
		{"malformed first array", `prefix [invalid] []`, nil},
		{"null fact", `[{"name":"太郎","facts":[null]}]`, nil},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := parsePersonFactsResponse(tc.response); !reflect.DeepEqual(got, tc.want) {
				t.Errorf("parsePersonFactsResponse = %#v, want %#v", got, tc.want)
			}
		})
	}
}
