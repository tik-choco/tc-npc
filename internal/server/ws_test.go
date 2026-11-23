package server

import (
	"context"
	"fmt"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/coder/websocket"
	"github.com/coder/websocket/wsjson"
	"github.com/google/uuid"
	"github.com/tik-choco/tc-npc/internal/bus"
)

func TestSpeechPayload(t *testing.T) {
	for _, tc := range []struct {
		name, content, speaker, id string
		transliterate              bool
		want                       object
	}{
		{"legacy", "こんにちは", "", "", false, object{"content": "こんにちは"}},
		{"blank speaker", "こんにちは", "   ", "", false, object{"content": "こんにちは"}},
		{"speaker", "こんにちは", "太郎", "", false, object{"content": "こんにちは", "speaker": "太郎"}},
		{"speaker and id", "こんにちは", "太郎", "req-1", false, object{"content": "こんにちは", "speaker": "太郎", "request_id": "req-1"}},
		{"id only", "こんにちは", "", "req-1", false, object{"content": "こんにちは", "request_id": "req-1"}},
		{"transliterate event", "Bobさんが新しくフォローしてくれました。", "Bob", "req-1", true,
			object{"content": "Bobさんが新しくフォローしてくれました。", "speaker": "Bob", "request_id": "req-1", "transliterate_name": true}},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := bus.SpeechPayload(tc.content, tc.speaker, tc.id, tc.transliterate); !reflect.DeepEqual(got, tc.want) {
				t.Fatalf("speech payload %v, want %v", got, tc.want)
			}
		})
	}
}

func TestBuildEventInstruction(t *testing.T) {
	taro := pointer("太郎")
	tier := pointer("Tier 1")
	for _, tc := range []struct {
		event  clientEvent
		detail string
	}{
		{clientEvent{Kind: "follow"}, "誰かさんが新しくフォローしてくれました。"},
		{clientEvent{Kind: "follow", UserName: taro}, "太郎さんが新しくフォローしてくれました。"},
		{clientEvent{Kind: "subscribe", UserName: taro, Tier: pointer("Tier 2")}, "太郎さんが新しくサブスク（Tier 2）してくれました。"},
		{clientEvent{Kind: "resub", UserName: taro, Amount: pointer(6.0), Tier: tier, Message: pointer("いつも見てます")}, "太郎さんがサブスクを更新してくれました（継続6ヶ月・Tier 1）。コメント:「いつも見てます」"},
		{clientEvent{Kind: "resub", UserName: taro, Amount: pointer(3.0), Tier: tier}, "太郎さんがサブスクを更新してくれました（継続3ヶ月・Tier 1）。"},
		{clientEvent{Kind: "gift", UserName: taro, Amount: pointer(5.0), Tier: tier}, "太郎さんがサブスク（Tier 1）を 5 件ギフトしてくれました。"},
		{clientEvent{Kind: "cheer", UserName: taro, Amount: pointer(300.0), Message: pointer("応援してます")}, "太郎さんが 300 bits を投げてくれました。コメント:「応援してます」"},
		{clientEvent{Kind: "raid", UserName: taro, Amount: pointer(42.0)}, "太郎さんが 42 人を連れてレイドしてくれました。"},
		{clientEvent{Kind: "points", UserName: taro, Message: pointer("こんにちは"), RewardTitle: pointer("たまごっちを見せて")}, "太郎さんがチャンネルポイント「たまごっちを見せて」を交換しました。入力:「こんにちは」"},
		{clientEvent{Kind: "donate", UserName: taro, Amount: pointer(500.0)}, "太郎さんから500円の投げ銭がありました。"},
		{clientEvent{Kind: "raidshield", UserName: taro, Text: pointer("何かが起きました")}, "raidshieldというイベントが発生しました: 何かが起きました。"},
		{clientEvent{Kind: "raidshield", UserName: taro}, "raidshieldというイベントが発生しました。"},
	} {
		line := buildEventInstruction(tc.event)
		parts := strings.Split(line, "\n")
		if len(parts) != 3 || parts[0] != "【配信イベント】" || parts[1] != tc.detail ||
			!strings.Contains(parts[2], "イベントの説明文をそのまま読み上げず、キャラクターとして反応してください") {
			t.Errorf("%s instruction: %q", tc.event.Kind, line)
		}
	}
}

func testSocketServer(t *testing.T, s *appState) *httptest.Server {
	t.Helper()
	s.startBusForwarder()
	httpServer := httptest.NewServer(s.routes())
	t.Cleanup(httpServer.Close)
	return httpServer
}

func dialSocket(t *testing.T, server *httptest.Server) *websocket.Conn {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	target := strings.Replace(server.URL, "http://", "ws://", 1) + "/ws"
	c, _, err := websocket.Dial(ctx, target, &websocket.DialOptions{
		HTTPHeader: http.Header{"Origin": {"https://client.example"}},
	})
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { _ = c.CloseNow() })
	return c
}

func writeWS(t *testing.T, c *websocket.Conn, data object) {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	if err := wsjson.Write(ctx, c, data); err != nil {
		t.Fatal(err)
	}
}

func readWS(t *testing.T, c *websocket.Conn, frameType string) object {
	t.Helper()
	ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	var value object
	if err := wsjson.Read(ctx, c, &value); err != nil {
		t.Fatalf("read %s: %v", frameType, err)
	}
	if value["type"] != frameType {
		t.Fatalf("expected %s, got %v", frameType, value)
	}
	return value
}

func startSocket(t *testing.T, srv *httptest.Server) *websocket.Conn {
	t.Helper()
	c := dialSocket(t, srv)
	readWS(t, c, "hello")
	readWS(t, c, "voice")
	return c
}

func TestWebSocketHelloRequestCorrelationAndVoice(t *testing.T) {
	s := testState(t)
	srv := testSocketServer(t, s)
	sub := s.mc.Bus.Subscribe()
	defer sub.Close()
	left, other := dialSocket(t, srv), startSocket(t, srv)
	hello := readWS(t, left, "hello")
	if hello["version"] != "1" || hello["character"] != nil || len(hello["modules"].(map[string]any)) != 7 {
		t.Fatalf("hello: %v", hello)
	}
	if _, present := hello["avatar"]; present {
		t.Fatalf("unresolved avatar: %v", hello)
	}
	if voice := readWS(t, left, "voice"); voice["active"] != true {
		t.Fatalf("initial voice state: %v", voice)
	}

	for _, tc := range []struct {
		busType string
		reply   object
		status  string
	}{
		{bus.MsgChatResponse, object{"content": "reply"}, "done"},
		{bus.MsgChatSilent, object{"reason": "declined"}, "done"},
		{bus.MsgChatError, object{"message": "upstream failed"}, "error"},
	} {
		writeWS(t, left, object{"type": "input", "text": "hello", "speaker": " 太郎 "})
		ack := readWS(t, left, "inputAccepted")
		id, _ := ack["requestId"].(string)
		if _, err := uuid.Parse(id); err != nil {
			t.Fatalf("ack request ID: %q", id)
		}
		if msg := receiveBus(t, sub); msg.Topic != bus.TopicSense || msg.Type != bus.MsgSpeech ||
			msg.Str("request_id") != id || msg.Str("speaker") != "太郎" || msg.Str("content") != "hello" {
			t.Fatalf("input bus envelope: %+v", msg)
		}
		for _, c := range []*websocket.Conn{left, other} {
			if chat := readWS(t, c, "chat"); chat["role"] != "user" || chat["text"] != "hello" {
				t.Fatalf("user chat: %v", chat)
			}
		}
		tc.reply["request_id"] = id
		s.mc.Bus.Publish(bus.TopicChat, tc.busType, tc.reply)
		for _, c := range []*websocket.Conn{left, other} {
			switch tc.busType {
			case bus.MsgChatResponse:
				chat := readWS(t, c, "chat")
				if chat["role"] != "assistant" || chat["text"] != "reply" {
					t.Fatalf("reply chat: %v", chat)
				}
			case bus.MsgChatSilent:
				if silent := readWS(t, c, "silent"); silent["reason"] != "declined" {
					t.Fatalf("silent frame: %v", silent)
				}
			}
			resp := readWS(t, c, "response")
			if resp["requestId"] != id || resp["status"] != tc.status {
				t.Fatalf("correlated response: %v", resp)
			}
			switch tc.busType {
			case bus.MsgChatResponse:
				if resp["text"] != "reply" || resp["message"] != nil {
					t.Fatalf("spoken response: %v", resp)
				}
			case bus.MsgChatSilent:
				if _, present := resp["text"]; present {
					t.Fatalf("silent response carried text: %v", resp)
				}
			case bus.MsgChatError:
				if resp["message"] != "upstream failed" || resp["text"] != nil {
					t.Fatalf("errored response: %v", resp)
				}
			}
		}
		if msg := receiveBus(t, sub); msg.Type != tc.busType {
			t.Fatalf("unexpected bus message: %+v", msg)
		}
	}

	writeWS(t, left, object{"type": "voiceStop"})
	if msg := receiveBus(t, sub); msg.Topic != bus.TopicInterrupt || msg.Type != bus.MsgVoiceStop {
		t.Fatalf("voice stop: %+v", msg)
	}
	for _, c := range []*websocket.Conn{left, other} {
		if voice := readWS(t, c, "voice"); voice["active"] != false {
			t.Fatalf("voice stop frame: %v", voice)
		}
	}
	reopened := dialSocket(t, srv)
	readWS(t, reopened, "hello")
	if voice := readWS(t, reopened, "voice"); voice["active"] != false {
		t.Fatalf("new client's voice state: %v", voice)
	}
	writeWS(t, left, object{"type": "voiceStart"})
	if msg := receiveBus(t, sub); msg.Type != bus.MsgVoiceStart {
		t.Fatalf("voice start: %+v", msg)
	}
	if voice := readWS(t, reopened, "voice"); voice["active"] != true {
		t.Fatalf("voice start frame: %v", voice)
	}
}

func TestWebSocketSuspendOwnersDisconnectAndReplay(t *testing.T) {
	s := testState(t)
	srv := testSocketServer(t, s)
	owner, viewer, unrelated := startSocket(t, srv), startSocket(t, srv), startSocket(t, srv)
	sub := s.mc.Bus.Subscribe()
	defer sub.Close()
	writeWS(t, owner, object{"type": "suspend"})
	readWS(t, owner, "suspendAccepted")
	if msg := receiveBus(t, sub); msg.Type != bus.MsgSuspend {
		t.Fatalf("suspend: %+v", msg)
	}
	if err := unrelated.Close(websocket.StatusNormalClosure, ""); err != nil {
		t.Fatal(err)
	}

	writeWS(t, viewer, object{"type": "input", "text": "queued"})
	writeWS(t, viewer, object{"type": "event", "kind": "follow", "userName": "Bob"})
	writeWS(t, viewer, object{"type": "interrupt"})
	readWS(t, viewer, "interruptAccepted")
	if msg := receiveBus(t, sub); msg.Type != bus.MsgInterrupt {
		t.Fatalf("input or event escaped the gate: %+v", msg)
	}

	if err := owner.Close(websocket.StatusNormalClosure, ""); err != nil {
		t.Fatal(err)
	}
	if msg := receiveBus(t, sub); msg.Type != bus.MsgResume {
		t.Fatalf("auto-resume: %+v", msg)
	}
	ack := readWS(t, viewer, "inputAccepted")
	if msg := receiveBus(t, sub); msg.Type != bus.MsgSpeech || msg.Str("content") != "queued" ||
		msg.Str("request_id") != ack["requestId"] {
		t.Fatalf("first replayed input: %+v", msg)
	}
	readWS(t, viewer, "chat")
	if ack := readWS(t, viewer, "eventAccepted"); ack["kind"] != "follow" {
		t.Fatalf("event ack: %v", ack)
	}
	event := readWS(t, viewer, "chat")
	if msg := receiveBus(t, sub); msg.Type != bus.MsgSpeech || msg.Str("content") != event["text"] ||
		msg.Str("speaker") != "Bob" || msg.Str("request_id") == "" || msg.Map()["transliterate_name"] != true {
		t.Fatalf("second replayed event: %+v", msg)
	}
}

func TestWebSocketSuspendOverflowResumeAndMalformedFrames(t *testing.T) {
	s := testState(t)
	srv := httptest.NewServer(s.routes())
	t.Cleanup(srv.Close)
	c := startSocket(t, srv)
	sub := s.mc.Bus.Subscribe()
	defer sub.Close()

	writeWS(t, c, object{"type": "input"})
	if got := readWS(t, c, "error"); !strings.HasPrefix(got["message"].(string), "invalid frame: ") {
		t.Fatalf("malformed frame error: %v", got)
	}
	writeWS(t, c, object{"type": "suspend"})
	readWS(t, c, "suspendAccepted")
	if msg := receiveBus(t, sub); msg.Type != bus.MsgSuspend {
		t.Fatalf("suspend: %+v", msg)
	}
	for i := 0; i <= suspendQueueCap; i++ {
		writeWS(t, c, object{"type": "input", "text": fmt.Sprint(i)})
	}
	writeWS(t, c, object{"type": "command", "text": "run"})
	if msg := receiveBus(t, sub); msg.Topic != bus.TopicAction || msg.Type != "command" || msg.Str("text") != "run" {
		t.Fatalf("expected ungated command: %+v", msg)
	}
	writeWS(t, c, object{"type": "resume"})
	readWS(t, c, "resumeAccepted")
	if msg := receiveBus(t, sub); msg.Type != bus.MsgResume {
		t.Fatalf("resume: %+v", msg)
	}
	for i := 1; i <= suspendQueueCap; i++ {
		ack := readWS(t, c, "inputAccepted")
		chat := readWS(t, c, "chat")
		msg := receiveBus(t, sub)
		if msg.Str("content") != fmt.Sprint(i) || chat["text"] != fmt.Sprint(i) ||
			msg.Str("request_id") != ack["requestId"] {
			t.Fatalf("replayed item %d: ack %v, chat %v, message %v", i, ack, chat, msg)
		}
	}
}
