package tui

import (
	"context"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync/atomic"
	"testing"
	"time"

	"github.com/coder/websocket"
)

func TestWebSocketReconnectSendsQueuedInputAndResumesReceiving(t *testing.T) {
	ctx, cancel := context.WithTimeout(t.Context(), 8*time.Second)
	defer cancel()
	var connections atomic.Int32
	sent := make(chan string, 1)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/ws" {
			t.Errorf("unexpected WebSocket path: %s", r.URL)
		}
		conn, err := websocket.Accept(w, r, nil)
		if err != nil {
			t.Error(err)
			return
		}
		defer conn.CloseNow()
		if connections.Add(1) == 1 {
			// Binary frames are ignored. A bad text frame is reported
			// before the eventual disconnect and retry.
			if err := conn.Write(ctx, websocket.MessageBinary, []byte("binary")); err != nil {
				t.Error(err)
			}
			if err := conn.Write(ctx, websocket.MessageText, []byte("not json")); err != nil {
				t.Error(err)
			}
			return
		}
		kind, data, err := conn.Read(ctx)
		if err != nil {
			t.Error(err)
			return
		}
		if kind != websocket.MessageText {
			t.Errorf("outgoing kind = %v, want text", kind)
		}
		select {
		case sent <- string(data):
		case <-ctx.Done():
			t.Error(ctx.Err())
		}
		if err := conn.Write(ctx, websocket.MessageText, []byte(`{"type":"inputAccepted","requestId":"r1"}`)); err != nil {
			t.Error(err)
		}
	}))
	defer server.Close()

	outgoing := &outgoingQueue{ready: make(chan struct{}, 1)}
	events := make(chan any, 16)
	done := make(chan struct{})
	go func() {
		defer close(done)
		reconnectLoop(ctx, "ws"+strings.TrimPrefix(server.URL, "http")+"/ws", server.Client(), events, outgoing)
	}()
	defer func() {
		cancel()
		<-done
	}()
	next := func() any {
		t.Helper()
		select {
		case event, ok := <-events:
			if ok {
				return event
			}
			t.Fatal("WebSocket event stream ended")
		case <-ctx.Done():
			t.Fatal(ctx.Err())
		}
		return nil
	}
	checkState := func(want connectionState) {
		t.Helper()
		if got := next(); got != want {
			t.Fatalf("connection transition = %+v, want %v", got, want)
		}
	}
	checkState(connecting)
	checkState(connected)
	if msg, ok := next().(serverMsg); !ok || msg.kind != "error" || msg.message == nil || !strings.Contains(*msg.message, "malformed frame") {
		t.Fatalf("missing malformed frame error: %+v", msg)
	}
	checkState(disconnected)
	outgoing.send(clientMsg{Text: "こんにちは"})
	checkState(reconnecting)
	checkState(connected)
	if msg, ok := next().(serverMsg); !ok || msg.kind != "inputAccepted" || msg.requestID != "r1" {
		t.Fatalf("missing acknowledgement after reconnect: %+v", msg)
	}
	select {
	case got := <-sent:
		if want := `{"type":"input","text":"こんにちは"}`; got != want {
			t.Errorf("queued message = %s, want %s", got, want)
		}
	case <-ctx.Done():
		t.Error(ctx.Err())
	}
}
