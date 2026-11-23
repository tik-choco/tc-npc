package mist

import (
	"log/slog"
	"slices"
	"strings"
	"time"

	"github.com/google/uuid"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/module"
)

const (
	helloInterval = 30 * time.Second
	hostTimeout   = 90 * time.Second
	mouthInterval = 34 * time.Millisecond
	requestPrefix = "avatar-link:"
)

type hostPresence struct {
	lastSeen      time.Time
	answeredHello bool
}

type avatarBridge struct {
	roomID     string
	joined     bool
	nodeID     string
	name       string
	allowHosts []string
	hosts      map[string]hostPresence
	ttsEnabled bool
	speaking   bool
	lastMouth  time.Time
}

func newAvatarBridge(mc module.Ctx, nodeID string) *avatarBridge {
	settings := mc.Config.Mist.AvatarLink
	name := strings.TrimSpace(settings.Name)
	if name == "" {
		if active, _ := character.ActiveCharacter(mc.DataDir, mc.Config.Character.ActiveID); active != nil {
			name = strings.TrimSpace(active.Sheet.Name)
		}
		if name == "" {
			name = "tc-npc"
		}
	}
	return &avatarBridge{
		roomID: strings.TrimSpace(settings.RoomID), nodeID: nodeID, name: name,
		allowHosts: slices.Clone(settings.AllowHosts), hosts: make(map[string]hostPresence),
		ttsEnabled: mc.Config.TTS.Enabled,
	}
}

func (a *avatarBridge) hello(e engine, now time.Time) {
	for id, host := range a.hosts {
		if now.Sub(host.lastSeen) >= hostTimeout {
			delete(a.hosts, id)
		}
	}
	a.send(e, brainMessage{Kind: "hello", Name: &a.name, Caps: avatarLinkCaps})
}

func (a *avatarBridge) send(e engine, message brainMessage) bool {
	ts := uint64(max(0, time.Now().UnixMilli()))
	data, err := encodeBrain(a.nodeID, uuid.New(), ts, message)
	if err != nil {
		slog.Debug("npc-mist: failed to encode Avatar Link message", "error", err)
		return false
	}
	e.broadcast(a.roomID, data, message.Kind == "mouth")
	return true
}

// receive refreshes presence and publishes accepted input. It reports whether
// to answer this host's hello.
func (a *avatarBridge) receive(b *bus.Bus, data []byte, now time.Time) bool {
	host, ok := decodeHost(data)
	if !ok || host.from == a.nodeID || (len(a.allowHosts) > 0 && !slices.Contains(a.allowHosts, host.from)) {
		return false
	}
	if host.kind == "bye" {
		delete(a.hosts, host.from)
		return false
	}
	presence := a.hosts[host.from]
	if now.Sub(presence.lastSeen) >= hostTimeout {
		presence.answeredHello = false
	}
	presence.lastSeen = now
	answerHello := host.kind == "hello" && !presence.answeredHello
	presence.answeredHello = presence.answeredHello || answerHello
	a.hosts[host.from] = presence
	if host.kind == "input" && (host.to == nil || *host.to == a.nodeID) {
		var speaker string
		if host.speaker != nil {
			speaker = *host.speaker
		}
		// Carry the input ID through npc-talk so overlapping turns retain
		// their individual reply correlations.
		b.Publish(bus.TopicSense, bus.MsgSpeech, bus.SpeechPayload(host.text, speaker, requestPrefix+host.id, false))
	}
	return answerHello
}

func (a *avatarBridge) fromBus(event bus.Message, now time.Time) brainMessage {
	if event.Topic == bus.TopicConfig && event.Type == bus.MsgConfigUpdated {
		tts, _ := event.Map()["tts"].(map[string]any)
		if enabled, ok := tts["enabled"].(bool); ok {
			a.ttsEnabled = enabled
		}
		return brainMessage{}
	}
	if !a.joined {
		return brainMessage{}
	}
	switch event.Topic {
	case bus.TopicChat:
		switch event.Type {
		case bus.MsgChatResponse:
			payload := event.Map()
			text := stringField(payload, "content")
			if text == nil {
				return brainMessage{}
			}
			audio := audioHost
			if a.ttsEnabled {
				audio = audioBrain
			}
			lineID := uuid.New()
			return brainMessage{Kind: "say", LineID: &lineID, Text: text, Audio: audio, InReplyTo: replyTo(payload)}
		case bus.MsgChatSilent:
			payload := event.Map()
			return brainMessage{Kind: "silent", InReplyTo: replyTo(payload), Reason: stringField(payload, "reason")}
		}
	case bus.TopicUI:
		switch event.Type {
		case "speaking":
			if active, ok := event.Map()["active"].(bool); ok {
				a.speaking = active
				return brainMessage{Kind: "speaking", Active: &active}
			}
		case "speaking_level":
			if a.speaking && (a.lastMouth.IsZero() || now.Sub(a.lastMouth) >= mouthInterval) {
				if level, ok := event.Map()["level"].(float64); ok {
					a.lastMouth = now
					if level < 0 {
						level = 0
					} else if level > 1 {
						level = 1
					}
					return brainMessage{Kind: "mouth", Level: mouthLevel(level)}
				}
			}
		}
	case bus.TopicInterrupt:
		switch event.Type {
		case bus.MsgInterrupt, bus.MsgSuspend, bus.MsgVoiceStop:
			return brainMessage{Kind: "stop"}
		}
	}
	return brainMessage{}
}

func stringField(payload map[string]any, key string) *string {
	if value, ok := payload[key].(string); ok {
		return &value
	}
	return nil
}

func replyTo(payload map[string]any) *string {
	requestID, _ := payload["request_id"].(string)
	if id, ok := strings.CutPrefix(requestID, requestPrefix); ok {
		return &id
	}
	return nil
}
