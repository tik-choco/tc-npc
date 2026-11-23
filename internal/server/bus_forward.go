package server

import (
	"encoding/json"
	"log/slog"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
)

const affectHistoryCap = 60 // Same cap as the browser's trend line.

func (s *appState) startBusForwarder() <-chan struct{} {
	sub := s.mc.Bus.Subscribe()
	done := make(chan struct{})
	go func() {
		defer close(done)
		defer sub.Close()
		volume := rateLimiter{interval: 100 * time.Millisecond}
		var dropped uint64
		for {
			select {
			case <-s.ctx.Done():
				return
			case msg, ok := <-sub.C:
				if !ok {
					return
				}
				if count := sub.Dropped(); count != dropped {
					slog.Warn("npc-server: bus forwarder lagged, dropped messages", "skipped", count-dropped)
					dropped = count
				}
				s.forwardBusMessage(&volume, msg)
			}
		}
	}()
	return done
}

func (s *appState) forwardBusMessage(volume *rateLimiter, msg bus.Message) {
	// The config topic carries real credentials. Only these four topics
	// participate in the browser protocol.
	switch msg.Topic {
	case bus.TopicChat, bus.TopicSense, bus.TopicMem, bus.TopicUI:
	default:
		return
	}
	payload, _ := readPayload(msg.Payload)
	switch msg.Topic {
	case bus.TopicChat:
		content := textField(payload)
		id, correlated := requestIDField(payload)
		switch msg.Type {
		case bus.MsgChatResponse:
			s.hub.broadcast(chatFrame{Type: "chat", Role: "assistant", Text: content, TS: nowMS()})
			if !correlated {
				id = "-"
			}
			// npc-speech emits ttsLine as each sentence starts playing.
			s.hub.broadcast(responseFrame{Type: "response", RequestID: id, Status: "done", Text: &content})
		case bus.MsgChatSilent:
			s.hub.broadcast(object{"type": "silent", "reason": stringField(payload, "reason"), "ts": nowMS()})
			// A silent turn also releases the client waiting on its input.
			if correlated {
				s.hub.broadcast(responseFrame{Type: "response", RequestID: id, Status: "done"})
			}
		case bus.MsgChatError:
			if correlated {
				message, ok := payload["message"].(string)
				if !ok {
					message = "chat turn failed"
				}
				s.hub.broadcast(responseFrame{Type: "response", RequestID: id, Status: "error", Message: &message})
			}
		}
	case bus.TopicSense:
		if msg.Type != bus.MsgVision && msg.Type != bus.MsgSpeech {
			return
		}
		content := textField(payload)
		if msg.Type == bus.MsgSpeech && s.echo.wasRecent(content) {
			return
		}
		s.hub.broadcast(object{"type": "sense", "kind": msg.Type, "text": content, "ts": nowMS()})
	case bus.TopicMem:
		kind, content := "long", textField(payload)
		switch msg.Type {
		case bus.MsgShortTermMemory:
			kind = "short"
			s.cacheMu.Lock()
			s.shortTerm = content
			s.cacheMu.Unlock()
		case bus.MsgLongTermMemory:
		default:
			return
		}
		s.hub.broadcast(object{"type": "memory", "kind": kind, "text": content})
	case bus.TopicUI:
		s.forwardUI(volume, msg, payload)
	}
}

func (s *appState) forwardUI(volume *rateLimiter, msg bus.Message, payload object) {
	switch msg.Type {
	case "volume":
		if volume.allow() {
			if level, ok := numberField(payload, "level"); ok {
				s.hub.broadcast(object{"type": "volume", "level": level})
			}
		}
	case "position":
		x, _ := numberField(payload, "x")
		y, _ := numberField(payload, "y")
		heading, _ := numberField(payload, "heading")
		s.hub.broadcast(object{"type": "position", "x": x, "y": y, "heading": heading})
	case "action_log":
		s.hub.broadcast(object{"type": "actionLog", "text": textField(payload)})
	case "speaking":
		// Dropping a playback edge could leave the mouth stuck open.
		if active, ok := payload["active"].(bool); ok {
			s.hub.broadcast(object{"type": "speaking", "active": active})
		}
	case "speaking_level":
		// Playback already throttles its reading at the source.
		if level, ok := numberField(payload, "level"); ok {
			s.hub.broadcast(object{"type": "speakingLevel", "level": level})
		}
	case "tts_line":
		s.hub.broadcast(object{"type": "ttsLine", "text": textField(payload)})
	case bus.MsgAffectState:
		var snapshot affectSnapshot
		if err := msg.Decode(&snapshot); err != nil {
			slog.Warn("npc-server: failed to parse affect_state payload", "error", err)
			return
		}
		entry := affectEntry{TS: nowMS(), affectSnapshot: snapshot}
		// Buffer before broadcasting. The shared timestamp lets the
		// browser de-duplicate a backfilled snapshot it already saw live.
		s.cacheMu.Lock()
		s.affectHistory = pushCapped(s.affectHistory, entry, affectHistoryCap)
		s.cacheMu.Unlock()
		s.hub.broadcast(affectFrame{Type: "affect", affectEntry: entry})
	case bus.MsgPersonUpdated:
		raw := msg.Payload
		if len(raw) == 0 {
			raw = json.RawMessage("null")
		}
		s.hub.broadcast(object{"type": "person", "person": raw})
	case bus.MsgPersonDeleted:
		if id, ok := payload["id"].(string); ok {
			s.hub.broadcast(object{"type": "personDeleted", "id": id})
		}
	case "translation":
		ts := nowMS()
		if number, ok := payload["ts"].(json.Number); ok {
			if n, err := number.Int64(); err == nil {
				ts = n
			}
		}
		reversed, _ := payload["reversed"].(bool)
		s.hub.broadcast(object{
			"type": "translation", "id": stringField(payload, "id"), "source": stringField(payload, "source"),
			"original": stringField(payload, "original"), "lang": stringField(payload, "lang"),
			"text": stringField(payload, "text"), "reversed": reversed, "ts": ts,
		})
	}
}

func pushCapped(history []affectEntry, entry affectEntry, cap int) []affectEntry {
	if cap <= 0 {
		return history
	}
	if len(history) >= cap {
		n := copy(history, history[len(history)-cap+1:])
		clear(history[n:])
		history = history[:n]
	}
	return append(history, entry)
}

func textField(payload object) string {
	if content, ok := payload["content"].(string); ok {
		return content
	}
	return stringField(payload, "text")
}

func stringField(payload object, key string) string {
	value, _ := payload[key].(string)
	return value
}

func numberField(payload object, key string) (float64, bool) {
	number, ok := payload[key].(json.Number)
	if !ok {
		return 0, false
	}
	value, err := number.Float64()
	if err != nil {
		return 0, false
	}
	return value, true
}

func requestIDField(payload object) (string, bool) {
	id, ok := payload["request_id"].(string)
	return id, ok
}
