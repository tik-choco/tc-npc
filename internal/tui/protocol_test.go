package tui

import (
	"encoding/json"
	"testing"
)

const modulesJSON = `{"talk":true,"memory":false,"speech":true,"vision":false,"action":true,"scheduler":false,"translation":false}`

func mustParseFrame(t *testing.T, raw string) serverMsg {
	t.Helper()
	var got serverMsg
	if err := json.Unmarshal([]byte(raw), &got); err != nil {
		t.Fatal(err)
	}
	return got
}

func TestParseHello(t *testing.T) {
	msg := mustParseFrame(t, `{"type":"hello","version":"1","modules":`+modulesJSON+`,"character":{"id":"c1","name":"Test"},"avatar":{"kind":"vrm","file":"a.vrm"}}`)
	if msg.unknown || msg.kind != "hello" || msg.version != "1" || !msg.modules.Talk || msg.modules.Memory ||
		msg.character == nil || msg.character.Name != "Test" {
		t.Fatalf("unexpected hello: %+v", msg)
	}
}

func TestParseIgnoresUnmodeledFields(t *testing.T) {
	msg := mustParseFrame(t, `{"type":"chat","role":"assistant","text":"hi","ts":1,"extraFieldFromANewerServer":true,"version":false}`)
	if msg.unknown || msg.kind != "chat" || msg.text == nil || *msg.text != "hi" {
		t.Fatalf("unexpected chat: %+v", msg)
	}
}

func TestParseUnknownKind(t *testing.T) {
	msg := mustParseFrame(t, `{"type":"speakingLevel","level":0.5}`)
	if !msg.unknown || msg.kind != "speakingLevel" {
		t.Fatalf("unexpected telemetry: %+v", msg)
	}
}

func TestInvalidFrameIsParseError(t *testing.T) {
	for _, raw := range []string{"not json at all", `{"role":"user","text":"hi"}`, `{"type":null}`, `{"type":42}`, `[]`} {
		var msg serverMsg
		if err := json.Unmarshal([]byte(raw), &msg); err == nil {
			t.Errorf("parsed %q successfully", raw)
		}
	}
}

func TestIncompatibleKnownPayloadFallsBackToUnknown(t *testing.T) {
	for kind, raw := range map[string]string{
		"chat":   `{"type":"chat","role":"assistant","text":"hi"}`,
		"hello":  `{"type":"hello","version":"1","modules":` + modulesJSON + `,"character":{"id":"c1"}}`,
		"status": `{"type":"status","modules":{"talk":true}}`,
		"error":  `{"type":"error","message":null}`,
	} {
		if msg := mustParseFrame(t, raw); !msg.unknown || msg.kind != kind {
			t.Errorf("%s decoded as %+v", kind, msg)
		}
	}
}

func TestParseSilent(t *testing.T) {
	msg := mustParseFrame(t, `{"type":"silent","reason":"declined","ts":123}`)
	if msg.unknown || msg.kind != "silent" || msg.reason != "declined" || msg.ts != 123 {
		t.Fatalf("unexpected silent: %+v", msg)
	}
}

func TestParseResponseError(t *testing.T) {
	msg := mustParseFrame(t, `{"type":"response","requestId":"r1","status":"error","message":"boom"}`)
	if msg.unknown || msg.kind != "response" || msg.requestID != "r1" ||
		msg.status != "error" || msg.text != nil || msg.message == nil || *msg.message != "boom" {
		t.Fatalf("unexpected response: %+v", msg)
	}
}

func TestMarshalClientInput(t *testing.T) {
	speaker := "op"
	for _, tc := range []struct {
		name string
		msg  clientMsg
		want string
	}{
		{"without speaker", clientMsg{Text: "こんにちは"}, `{"type":"input","text":"こんにちは"}`},
		{"with speaker", clientMsg{Text: "hi", Speaker: &speaker}, `{"type":"input","text":"hi","speaker":"op"}`},
	} {
		t.Run(tc.name, func(t *testing.T) {
			out, err := json.Marshal(tc.msg)
			if err != nil || string(out) != tc.want {
				t.Fatalf("marshal = %s, %v, want %s", out, err, tc.want)
			}
		})
	}
}
