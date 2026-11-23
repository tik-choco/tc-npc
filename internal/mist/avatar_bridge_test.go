package mist

import (
	"encoding/json"
	"fmt"
	"reflect"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
)

func testBridge() *avatarBridge {
	return &avatarBridge{
		roomID: "room", joined: true, nodeID: "brain-1", name: "NPC",
		allowHosts: []string{"allowed"}, hosts: make(map[string]hostPresence),
	}
}

func hostMessage(kind, from, fields string) []byte {
	return fmt.Appendf(nil, `{"v":1,"t":%q,"id":"i1","from":%q,"role":"host","ts":1%s}`, kind, from, fields)
}

func busMessage(topic, kind, payload string) bus.Message {
	return bus.Message{Topic: topic, Type: kind, Payload: json.RawMessage(payload)}
}

func TestAcceptsOnlyAllowedInputForThisBrainAndKeepsItsID(t *testing.T) {
	brain := testBridge()
	b := bus.New()
	rx := b.Subscribe()
	defer rx.Close()
	now := time.Now()
	for _, route := range [][2]string{{"other", "brain-1"}, {"allowed", "brain-2"}, {"allowed", "brain-1"}} {
		input := hostMessage("input", route[0], fmt.Sprintf(`,"text":"Hi","speaker":"Viewer","to":%q`, route[1]))
		brain.receive(b, input, now)
	}
	select {
	case input := <-rx.C:
		want := map[string]any{"content": "Hi", "speaker": "Viewer", "request_id": "avatar-link:i1"}
		if input.Topic != bus.TopicSense || input.Type != bus.MsgSpeech || !reflect.DeepEqual(input.Map(), want) {
			t.Fatalf("unexpected input: %+v", input)
		}
	default:
		t.Fatal("no input was published")
	}
	select {
	case input := <-rx.C:
		t.Fatalf("unexpected additional input: %+v", input)
	default:
	}
	response := busMessage(bus.TopicChat, bus.MsgChatResponse, `{"content":"Hello","request_id":"avatar-link:i1"}`)
	if say := brain.fromBus(response, now); say.Kind != "say" || say.Audio != audioHost ||
		say.InReplyTo == nil || *say.InReplyTo != "i1" {
		t.Fatalf("unexpected host-audio reply: %+v", say)
	}
	update := busMessage(bus.TopicConfig, bus.MsgConfigUpdated, `{"tts":{"enabled":true}}`)
	if message := brain.fromBus(update, now); message.Kind != "" {
		t.Fatalf("unexpected config response: %+v", message)
	}
	if say := brain.fromBus(response, now); say.Kind != "say" || say.Audio != audioBrain {
		t.Fatalf("unexpected brain-audio reply: %+v", say)
	}
	unrelated := busMessage(bus.TopicChat, bus.MsgChatResponse, `{"content":"Hello","request_id":"ws-request"}`)
	if say := brain.fromBus(unrelated, now); say.Kind != "say" || say.InReplyTo != nil {
		t.Fatalf("unexpected unrelated reply: %+v", say)
	}
	silent := busMessage(bus.TopicChat, bus.MsgChatSilent, `{"request_id":"avatar-link:i2","reason":"declined"}`)
	if reply := brain.fromBus(silent, now); reply.Kind != "silent" || reply.InReplyTo == nil || *reply.InReplyTo != "i2" ||
		reply.Reason == nil || *reply.Reason != "declined" {
		t.Fatalf("unexpected silent reply: %+v", reply)
	}
}

func TestAnswersOneHelloPerHostPresence(t *testing.T) {
	brain, b := testBridge(), bus.New()
	hello := hostMessage("hello", "allowed", `,"name":"Host","caps":[]`)
	state := hostMessage("state", "allowed", `,"speaking":false`)
	bye := hostMessage("bye", "allowed", "")
	now := time.Now()
	for _, step := range []struct {
		data []byte
		at   time.Duration
		want bool
	}{
		{hello, 0, true}, {hello, 0, false}, {hello, hostTimeout, true},
		{state, 2*hostTimeout - time.Second, false}, {hello, 2 * hostTimeout, false},
		{bye, 2 * hostTimeout, false}, {hello, 2 * hostTimeout, true},
	} {
		if got := brain.receive(b, step.data, now.Add(step.at)); got != step.want {
			t.Errorf("receive %s at %v = %v, want %v", step.data, step.at, got, step.want)
		}
	}
}

func TestThrottlesMouthWhileSpeaking(t *testing.T) {
	brain := testBridge()
	now := time.Now()
	level := busMessage(bus.TopicUI, "speaking_level", `{"level":1.5}`)
	start := busMessage(bus.TopicUI, "speaking", `{"active":true}`)
	stop := busMessage(bus.TopicUI, "speaking", `{"active":false}`)
	for _, step := range []struct {
		message bus.Message
		at      time.Duration
		kind    string
	}{
		{level, 0, ""}, {start, 0, "speaking"}, {level, 0, "mouth"},
		{level, 33 * time.Millisecond, ""}, {level, mouthInterval, "mouth"},
		{stop, mouthInterval, "speaking"}, {level, 2 * mouthInterval, ""},
	} {
		got := brain.fromBus(step.message, now.Add(step.at))
		if got.Kind != step.kind {
			t.Errorf("fromBus %s at %v = %+v, want %q", step.message.Payload, step.at, got, step.kind)
		}
		if got.Kind == "mouth" && got.Level != "1.0" {
			t.Errorf("mouth level = %s, want 1.0", got.Level)
		}
		if got.Kind == "speaking" {
			active, _ := step.message.Map()["active"].(bool)
			if got.Active == nil || *got.Active != active {
				t.Errorf("unexpected speaking flag for %s: %+v", step.message.Payload, got)
			}
		}
	}
}
