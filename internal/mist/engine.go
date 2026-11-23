package mist

import (
	"context"
	"log/slog"
	"strings"
	"sync/atomic"
)

const (
	eventRaw            = 0
	eventRoomJoined     = 10
	eventRoomJoinFailed = 11
)

type engine interface {
	start(nodeID, signalingURL string) (*engineEvents, error)
	joinRoom(room string)
	broadcast(room string, data []byte, unreliable bool)
	storageGet(ctx context.Context, cid string) ([]byte, error)
	leaveAllRooms()
}

type roomEvent struct {
	kind uint32
	room string
	data []byte
	next *roomEvent
}

type nativeLog struct {
	level   slog.Level
	message string
}

type engineEvents struct {
	ready   chan struct{}
	pending atomic.Pointer[roomEvent]
	logs    chan nativeLog
	head    *roomEvent // owned by the receiving goroutine
}

func newEngineEvents() *engineEvents {
	return &engineEvents{ready: make(chan struct{}, 1), logs: make(chan nativeLog, 64)}
}

// The DLL dispatch thread must never wait for Go to handle an event. Keep the
// events in a queue so a burst of raw messages cannot drop room readiness.
func (e *engineEvents) push(event *roomEvent) {
	for {
		event.next = e.pending.Load()
		if e.pending.CompareAndSwap(event.next, event) {
			break
		}
	}
	e.signal()
}

func (e *engineEvents) signal() {
	select {
	case e.ready <- struct{}{}:
	default:
	}
}

// Pop one event per select iteration so room traffic cannot monopolize the
// bus and heartbeat loop. A callback can coalesce its signal with ours.
func (e *engineEvents) pop() *roomEvent {
	if e.head == nil {
		e.head = e.drain()
	}
	event := e.head
	if event != nil {
		e.head = event.next
		event.next = nil
	}
	if e.head != nil || e.pending.Load() != nil {
		e.signal()
	}
	return event
}

func (e *engineEvents) drain() *roomEvent {
	var first *roomEvent
	for event := e.pending.Swap(nil); event != nil; {
		next := event.next
		event.next = first
		first = event
		event = next
	}
	return first
}

// Pass flat config overrides to init_with_config. The DLL fills in the same
// Config::new_default settings as the Rust engine, including relay resolution.
type engineConfig struct {
	SignalingURL string          `json:"signalingUrl,omitempty"`
	Signaling    signalingConfig `json:"signaling"`
}

type signalingConfig struct {
	Mode  string                `json:"mode"`
	Nostr *nostrSignalingConfig `json:"nostr"`
}

type nostrSignalingConfig struct {
	Relays        []string `json:"relays"`
	DiscoveryKind uint32   `json:"discoveryKind"`
	MessageKind   uint32   `json:"messageKind"`
	TTLSeconds    uint64   `json:"ttlSeconds"`
	InviteSalt    string   `json:"inviteSalt"`
	InviteCode    string   `json:"inviteCode"`
}

func makeEngineConfig(signalingURL string) engineConfig {
	if signalingURL = strings.TrimSpace(signalingURL); signalingURL != "" {
		return engineConfig{SignalingURL: signalingURL, Signaling: signalingConfig{Mode: "websocket"}}
	}
	return engineConfig{Signaling: signalingConfig{
		Mode: "nostr",
		Nostr: &nostrSignalingConfig{
			Relays: []string{}, DiscoveryKind: 25049, MessageKind: 25050, TTLSeconds: 600,
			InviteSalt: "tik-choco-v1", InviteCode: "tik-choco-public-v1",
		},
	}}
}
