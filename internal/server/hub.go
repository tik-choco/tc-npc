package server

import (
	"bytes"
	"encoding/json"
	"log/slog"
	"slices"
	"sync"
	"time"
)

type wsClient struct {
	wake  chan struct{}
	queue [][]byte // guarded by hub.mu
}

// hub fans each serialized frame out to independent FIFO queues. A slow
// socket cannot block the bus forwarder or another socket's writer.
type hub struct {
	mu      sync.Mutex
	nextID  uint64
	clients map[uint64]*wsClient
}

func (h *hub) register(initial ...any) (uint64, *wsClient) {
	c := &wsClient{wake: make(chan struct{}, 1)}
	for _, frame := range initial {
		if data := marshalFrame(frame); data != nil {
			c.enqueue(data)
		}
	}
	h.mu.Lock()
	defer h.mu.Unlock()
	if h.clients == nil {
		h.clients = make(map[uint64]*wsClient)
	}
	id := h.nextID
	h.nextID++
	h.clients[id] = c
	return id, c
}

func (h *hub) unregister(id uint64) {
	h.mu.Lock()
	delete(h.clients, id)
	h.mu.Unlock()
}

func marshalFrame(frame any) []byte {
	var data bytes.Buffer
	enc := json.NewEncoder(&data)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(frame); err != nil {
		slog.Warn("npc-server: failed to serialize ws frame", "error", err)
		return nil
	}
	return bytes.TrimSuffix(data.Bytes(), []byte{10})
}

func (c *wsClient) enqueue(data []byte) {
	c.queue = append(c.queue, data)
	select {
	case c.wake <- struct{}{}:
	default:
	}
}

func (h *hub) take(c *wsClient) [][]byte {
	h.mu.Lock()
	defer h.mu.Unlock()
	queue := c.queue
	c.queue = nil
	return queue
}

func (h *hub) sendTo(id uint64, frame any) {
	if data := marshalFrame(frame); data != nil {
		h.mu.Lock()
		defer h.mu.Unlock()
		if c := h.clients[id]; c != nil {
			c.enqueue(data)
		}
	}
}

func (h *hub) broadcast(frame any) {
	if data := marshalFrame(frame); data != nil {
		h.mu.Lock()
		defer h.mu.Unlock()
		for _, c := range h.clients {
			c.enqueue(data)
		}
	}
}

const (
	echoWindow     = time.Second
	echoMaxEntries = 32
)

type echoEntry struct {
	text string
	at   time.Time
}

// echoGuard suppresses speech that the server already echoed as user chat.
type echoGuard struct {
	mu     sync.Mutex
	recent []echoEntry
}

func (g *echoGuard) mark(text string) {
	g.mu.Lock()
	defer g.mu.Unlock()
	if len(g.recent) == echoMaxEntries {
		g.recent = slices.Delete(g.recent, 0, 1)
	}
	g.recent = append(g.recent, echoEntry{text, time.Now()})
}

func (g *echoGuard) wasRecent(text string) bool {
	g.mu.Lock()
	defer g.mu.Unlock()
	now := time.Now()
	g.recent = slices.DeleteFunc(g.recent, func(e echoEntry) bool { return now.Sub(e.at) > echoWindow })
	for i, e := range g.recent {
		if e.text == text {
			g.recent = slices.Delete(g.recent, i, i+1)
			return true
		}
	}
	return false
}

// rateLimiter is local to the bus forwarder. Mic volume shares one budget
// across clients, independently of voice playback readings.
type rateLimiter struct {
	interval time.Duration
	last     time.Time
}

func (l *rateLimiter) allow() bool {
	now := time.Now()
	if now.Sub(l.last) < l.interval {
		return false
	}
	l.last = now
	return true
}
