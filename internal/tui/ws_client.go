package tui

import (
	"context"
	"encoding/json"
	"net/http"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/coder/websocket"
)

const (
	reconnectMin = time.Second
	reconnectMax = 5 * time.Second
)

// outgoingQueue holds messages typed during an outage until the connection
// comes back. The wake-up channel lets the UI enqueue without waiting on
// network I/O or on the reconnect delay.
type outgoingQueue struct {
	mu    sync.Mutex
	items []clientMsg
	ready chan struct{}
}

func (q *outgoingQueue) send(msg clientMsg) {
	q.mu.Lock()
	q.items = append(q.items, msg)
	q.mu.Unlock()
	select {
	case q.ready <- struct{}{}:
	default:
	}
}

func (q *outgoingQueue) pop() (clientMsg, bool) {
	q.mu.Lock()
	defer q.mu.Unlock()
	if len(q.items) == 0 {
		return clientMsg{}, false
	}
	msg := q.items[0]
	q.items[0] = clientMsg{}
	q.items = q.items[1:]
	if len(q.items) == 0 {
		q.items = nil
	}
	return msg, true
}

func reconnectLoop(ctx context.Context, endpoint string, httpClient *http.Client, events chan<- any, outgoing *outgoingQueue) {
	defer close(events)
	next := connecting
	backoff := reconnectMin
	for ctx.Err() == nil {
		if !postEvent(ctx, events, next) {
			return
		}
		conn, _, err := websocket.Dial(ctx, endpoint, &websocket.DialOptions{HTTPClient: httpClient})
		if err != nil {
			if !postEvent(ctx, events, serverError("connect failed: "+err.Error())) {
				return
			}
		} else {
			backoff = reconnectMin
			next = reconnecting
			if postEvent(ctx, events, connected) {
				runConnection(ctx, conn, events, outgoing)
			}
			_ = conn.CloseNow()
			if !postEvent(ctx, events, disconnected) {
				return
			}
		}

		timer := time.NewTimer(backoff)
		select {
		case <-ctx.Done():
			timer.Stop()
			return
		case <-timer.C:
		}
		backoff = min(backoff*2, reconnectMax)
	}
}

func serverError(message string) serverMsg {
	return serverMsg{kind: "error", message: &message}
}

func postEvent(ctx context.Context, events chan<- any, event any) bool {
	select {
	case events <- event:
		return true
	case <-ctx.Done():
		return false
	}
}

func runConnection(ctx context.Context, conn *websocket.Conn, events chan<- any, outgoing *outgoingQueue) {
	// Match the Rust websocket client's message limit. The library default
	// is much smaller and would disconnect on an ordinary long reply.
	conn.SetReadLimit(64 << 20)

	ctx, cancel := context.WithCancel(ctx)
	done := make(chan struct{})
	go func() {
		defer close(done)
		defer cancel() // A write failure also ends the read and triggers reconnect.
		writeFrames(ctx, conn, outgoing)
	}()
	defer func() {
		cancel()
		<-done
	}()

	for {
		kind, data, err := conn.Read(ctx)
		if err != nil {
			return
		}
		if kind != websocket.MessageText {
			continue
		}
		if !utf8.Valid(data) {
			return
		}
		var msg serverMsg
		if err := json.Unmarshal(data, &msg); err != nil {
			msg = serverError("malformed frame: " + err.Error())
		}
		if !postEvent(ctx, events, msg) {
			return
		}
	}
}

func writeFrames(ctx context.Context, conn *websocket.Conn, outgoing *outgoingQueue) {
	for ctx.Err() == nil {
		msg, ok := outgoing.pop()
		if !ok {
			select {
			case <-outgoing.ready:
				continue
			case <-ctx.Done():
				return
			}
		}
		data, err := json.Marshal(msg)
		if err == nil {
			// Once a write has been attempted, do not replay it on reconnect:
			// the server may already have received and acted on it.
			if conn.Write(ctx, websocket.MessageText, data) != nil {
				return
			}
		}
	}
}
