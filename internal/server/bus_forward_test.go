package server

import (
	"fmt"
	"net/http"
	"reflect"
	"testing"

	"github.com/tik-choco/tc-npc/internal/bus"
)

func TestBusTextAndRequestIDFields(t *testing.T) {
	for _, tc := range []struct {
		name       string
		payload    object
		text, id   string
		correlated bool
	}{
		{"prefer content", object{"content": "c", "text": "t"}, "c", "", false},
		{"fallback text", object{"text": "t"}, "t", "", false},
		{"tagged id", object{"content": "c", "request_id": "req-1"}, "c", "req-1", true},
		{"missing id", object{"content": "c"}, "c", "", false},
	} {
		t.Run(tc.name, func(t *testing.T) {
			id, ok := requestIDField(tc.payload)
			if id != tc.id || ok != tc.correlated || textField(tc.payload) != tc.text {
				t.Fatalf("unexpected decoded payload: %v", tc.payload)
			}
		})
	}
}

func affectEntries(timestamps ...int64) []affectEntry {
	out := make([]affectEntry, 0, len(timestamps))
	for _, ts := range timestamps {
		out = append(out, affectEntry{TS: ts})
	}
	return out
}

func TestPushCapped(t *testing.T) {
	for _, tc := range []struct {
		name               string
		initial, add, want []affectEntry
		cap                int
	}{
		{"under cap", nil, affectEntries(1, 2, 3), affectEntries(1, 2, 3), 5},
		{"overflow", nil, affectEntries(1, 2, 3, 4, 5), affectEntries(3, 4, 5), 3},
		{"zero cap", nil, affectEntries(1), nil, 0},
		{"already over cap", affectEntries(1, 2, 3, 4), affectEntries(5), affectEntries(4, 5), 2},
	} {
		t.Run(tc.name, func(t *testing.T) {
			for _, add := range tc.add {
				tc.initial = pushCapped(tc.initial, add, tc.cap)
			}
			if !reflect.DeepEqual(tc.initial, tc.want) {
				t.Fatalf("history %v, want %v", tc.initial, tc.want)
			}
		})
	}
}

func TestRecentAffectEntries(t *testing.T) {
	for _, tc := range []struct {
		name          string
		history, want []affectEntry
		limit         int
	}{
		{"all", affectEntries(1, 2, 3), affectEntries(1, 2, 3), 10},
		{"tail", affectEntries(1, 2, 3, 4, 5), affectEntries(4, 5), 2},
		{"zero", affectEntries(1, 2, 3), affectEntries(), 0},
		{"empty", nil, affectEntries(), 60},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := recentAffectEntries(tc.history, tc.limit); !reflect.DeepEqual(got, tc.want) {
				t.Fatalf("history %v, want %v", got, tc.want)
			}
		})
	}
}

func TestBusFramesAndRESTCaches(t *testing.T) {
	s := testState(t)
	c := startSocket(t, testSocketServer(t, s))
	s.mc.Bus.Publish(bus.TopicConfig, bus.MsgConfigUpdated, object{"api_key": "private"})
	s.mc.Bus.Publish(bus.TopicChat, bus.MsgChatError, object{"message": "no requester"})
	for _, tc := range []struct {
		topic, message string
		payload, frame object
	}{
		{bus.TopicSense, bus.MsgVision, object{"content": "seen"}, object{"type": "sense", "kind": "vision", "text": "seen"}},
		{bus.TopicSense, bus.MsgSpeech, object{"text": "heard"}, object{"type": "sense", "kind": "speech", "text": "heard"}},
		{bus.TopicMem, bus.MsgShortTermMemory, object{"content": "summary"}, object{"type": "memory", "kind": "short", "text": "summary"}},
		{bus.TopicMem, bus.MsgLongTermMemory, object{"content": "record"}, object{"type": "memory", "kind": "long", "text": "record"}},
		{bus.TopicUI, "action_log", object{"text": "finished"}, object{"type": "actionLog", "text": "finished"}},
		{bus.TopicUI, "position", object{"x": 4, "y": -2}, object{"type": "position", "x": 4.0, "y": -2.0, "heading": 0.0}},
		{bus.TopicUI, "volume", object{"level": .5}, object{"type": "volume", "level": .5}},
		{bus.TopicUI, "speaking", object{"active": true}, object{"type": "speaking", "active": true}},
		{bus.TopicUI, "speaking_level", object{"level": .8}, object{"type": "speakingLevel", "level": .8}},
		{bus.TopicUI, "speaking_level", object{"level": 0}, object{"type": "speakingLevel", "level": 0.0}},
		{bus.TopicUI, "speaking", object{"active": false}, object{"type": "speaking", "active": false}},
		{bus.TopicUI, "tts_line", object{"content": "sentence"}, object{"type": "ttsLine", "text": "sentence"}},
		{bus.TopicUI, "translation", object{"id": "x", "source": "user", "original": "hi", "lang": "ja", "text": "やあ", "reversed": true, "ts": 7},
			object{"type": "translation", "id": "x", "source": "user", "original": "hi", "lang": "ja", "text": "やあ", "reversed": true, "ts": 7.0}},
		{bus.TopicUI, bus.MsgPersonUpdated, object{"id": "p"}, object{"type": "person", "person": object{"id": "p"}}},
		{bus.TopicUI, bus.MsgPersonDeleted, object{"id": "p"}, object{"type": "personDeleted", "id": "p"}},
	} {
		s.mc.Bus.Publish(tc.topic, tc.message, tc.payload)
		frame := readWS(t, c, tc.frame["type"].(string))
		if tc.message == bus.MsgVision || tc.message == bus.MsgSpeech {
			if ts, ok := frame["ts"].(float64); !ok || ts <= 0 {
				t.Errorf("sense timestamp: %v", frame)
			}
			delete(frame, "ts")
		}
		if !reflect.DeepEqual(frame, tc.frame) {
			t.Fatalf("%s frame %v, want %v", tc.message, frame, tc.frame)
		}
	}

	s.mc.Bus.Publish(bus.TopicChat, bus.MsgChatResponse, object{"content": "unprompted"})
	readWS(t, c, "chat")
	if frame := readWS(t, c, "response"); frame["requestId"] != "-" || frame["text"] != "unprompted" {
		t.Fatalf("self-initiated response: %v", frame)
	}

	h := s.routes()
	w := request(t, h, http.MethodGet, "/api/memory", "", http.StatusOK)
	if w.Body.String() != `{"longTerm":[],"shortTerm":"summary"}` {
		t.Fatalf("short-term cache: %s", w.Body)
	}
	for i := 0; i <= affectHistoryCap; i++ {
		s.mc.Bus.Publish(bus.TopicUI, bus.MsgAffectState, object{
			"familiarity": .5, "partner": fmt.Sprint(i), "partnerKnown": true,
			"drives": []object{{"key": "substance_p", "level": .25, "base": .5}},
		})
	}
	var last object
	for i := 0; i <= affectHistoryCap; i++ {
		last = readWS(t, c, "affect")
	}
	delete(last, "type")
	w = request(t, h, http.MethodGet, "/api/affect/history?limit=999", "", http.StatusOK)
	entries := decodeResponse[map[string][]object](t, w.Body.Bytes())["entries"]
	if len(entries) != affectHistoryCap || entries[0]["partner"] != "1" ||
		!reflect.DeepEqual(entries[len(entries)-1], last) {
		t.Fatalf("history length %d or history differs from live frame %v", len(entries), last)
	}
}
