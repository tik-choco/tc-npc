// Package bus is the in-process publish/subscribe bus that replaces the
// Redis pub/sub layer of the original Go agent-* services (and the
// tokio::sync::broadcast bus of the Rust port). The envelope shape
// ({"type": ..., "payload": ...}) and the topic/type string constants are
// identical to both predecessors.
package bus

import (
	"encoding/json"
	"log/slog"
	"strings"
	"sync"
	"sync/atomic"
)

// ChannelCapacity is the per-subscriber buffer size. A subscriber that falls
// further behind than this loses messages (like a lagged tokio broadcast
// receiver) instead of blocking publishers.
const ChannelCapacity = 256

// Envelope is the wire shape every module publishes/subscribes with.
type Envelope struct {
	Type    string          `json:"type"`
	Payload json.RawMessage `json:"payload"`
}

// Message is an envelope as delivered to subscribers, tagged with the topic
// it was published on. Subscribers receive every topic and filter as needed.
type Message struct {
	Topic   string
	Type    string
	Payload json.RawMessage
}

// Decode unmarshals the payload into v.
func (m Message) Decode(v any) error {
	if len(m.Payload) == 0 {
		return json.Unmarshal([]byte("null"), v)
	}
	return json.Unmarshal(m.Payload, v)
}

// Map decodes the payload as a JSON object. A non-object payload yields an
// empty (non-nil) map.
func (m Message) Map() map[string]any {
	out := map[string]any{}
	_ = json.Unmarshal(m.Payload, &out)
	if out == nil {
		out = map[string]any{}
	}
	return out
}

// Str returns payload[key] when the payload is an object and that field is a
// string, else "".
func (m Message) Str(key string) string {
	s, _ := m.Map()[key].(string)
	return s
}

// Bus is safe for concurrent use. The zero value is not usable; use New.
type Bus struct {
	mu   sync.RWMutex
	subs map[*Subscription]struct{}
}

// Subscription receives every message published after it was created.
type Subscription struct {
	C       <-chan Message
	ch      chan Message
	bus     *Bus
	once    sync.Once
	dropped atomic.Uint64
}

// New creates an empty bus.
func New() *Bus {
	return &Bus{subs: map[*Subscription]struct{}{}}
}

// Publish serializes payload and delivers it to every current subscriber.
// It never blocks: a subscriber whose buffer is full misses the message
// (counted in Dropped), matching the fire-and-forget semantics of the Go bus
// client's Publish. A payload that fails to serialize is logged and dropped.
// A json.RawMessage or []byte payload is used verbatim; nil becomes null.
func (b *Bus) Publish(topic, msgType string, payload any) {
	raw, err := marshal(payload)
	if err != nil {
		slog.Warn("bus: failed to serialize payload", "topic", topic, "type", msgType, "error", err)
		return
	}
	msg := Message{Topic: topic, Type: msgType, Payload: raw}
	b.mu.RLock()
	defer b.mu.RUnlock()
	for s := range b.subs {
		select {
		case s.ch <- msg:
		default:
			s.dropped.Add(1)
		}
	}
}

func marshal(payload any) (json.RawMessage, error) {
	switch p := payload.(type) {
	case nil:
		return json.RawMessage("null"), nil
	case json.RawMessage:
		if len(p) == 0 {
			return json.RawMessage("null"), nil
		}
		return p, nil
	case []byte:
		if len(p) == 0 {
			return json.RawMessage("null"), nil
		}
		return json.RawMessage(p), nil
	}
	b, err := json.Marshal(payload)
	if err != nil {
		return nil, err
	}
	return b, nil
}

// Subscribe registers a new subscriber. Call Close when done.
func (b *Bus) Subscribe() *Subscription {
	ch := make(chan Message, ChannelCapacity)
	s := &Subscription{C: ch, ch: ch, bus: b}
	b.mu.Lock()
	b.subs[s] = struct{}{}
	b.mu.Unlock()
	return s
}

// Close unregisters the subscription and closes C. Safe to call repeatedly.
func (s *Subscription) Close() {
	s.once.Do(func() {
		s.bus.mu.Lock()
		delete(s.bus.subs, s)
		s.bus.mu.Unlock()
		close(s.ch)
	})
}

// Dropped reports how many messages this subscriber missed because its
// buffer was full.
func (s *Subscription) Dropped() uint64 { return s.dropped.Load() }

// SpeechPayload builds an agent:sense / speech payload for an operator or
// external client. npc-talk echoes request_id on the resulting reply,
// silence, or error. transliterateName opts stream-event viewer handles into
// katakana conversion. Empty speaker / requestID are omitted.
func SpeechPayload(content, speaker, requestID string, transliterateName bool) map[string]any {
	p := map[string]any{"content": content}
	if s := strings.TrimSpace(speaker); s != "" {
		p["speaker"] = s
	}
	if requestID != "" {
		p["request_id"] = requestID
	}
	if transliterateName {
		p["transliterate_name"] = true
	}
	return p
}

// Topics: canonical channel names shared across the former agent-* services.
const (
	TopicChat      = "agent:chat"
	TopicMem       = "agent:mem"
	TopicSense     = "agent:sense"
	TopicInterrupt = "agent:interrupt"
	TopicAction    = "agent:action"
	// TopicConfig carries MsgConfigUpdated after the config is persisted.
	// Payload is the complete new Config, UNREDACTED — this topic must never
	// be forwarded to WS clients.
	TopicConfig = "npc:config"
	// TopicUI carries UI-only updates consumed by the server's bus_forward,
	// which relays recognized envelopes to connected WS clients.
	TopicUI = "npc:ui"
)

// Envelope type values.
const (
	MsgChatResponse = "chat_response"
	// MsgChatSilent: npc-talk answered by not speaking. Payload
	// {"reason", "input"} (+ request_id when the turn had a requester).
	MsgChatSilent = "chat_silent"
	// MsgChatError: a turn failed outright. Payload {"request_id", "message"};
	// only published when the turn had a requester.
	MsgChatError       = "chat_error"
	MsgChatLog         = "chat_log"
	MsgShortTermMemory = "short_term_memory"
	MsgLongTermMemory  = "long_term_memory"
	MsgSpeech          = "speech"
	MsgVision          = "vision"
	MsgAction          = "action"
	MsgTTS             = "tts"
	MsgSuspend         = "suspend"
	MsgResume          = "resume"
	// MsgInterrupt: one-shot "stop what you're doing" on TopicInterrupt.
	MsgInterrupt = "interrupt"
	// MsgVoiceStart / MsgVoiceStop: operator master switch for the cascade
	// voice loop, on TopicInterrupt. Never expires (unlike suspend/resume).
	MsgVoiceStart    = "voice_start"
	MsgVoiceStop     = "voice_stop"
	MsgConfigUpdated = "config_updated"
	// MsgAffectState: npc-talk's AffectSnapshot after every update, on TopicUI.
	MsgAffectState = "affect_state"
	// MsgPersonSeen: npc-vision person observation, on TopicSense.
	MsgPersonSeen = "person_seen"
	// MsgPersonMemory: npc-memory person profile, on TopicMem.
	MsgPersonMemory = "person_memory"
	// MsgPersonUpdated / MsgPersonDeleted: on TopicUI.
	MsgPersonUpdated = "person_updated"
	MsgPersonDeleted = "person_deleted"
)
