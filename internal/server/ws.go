package server

import (
	"context"
	"fmt"
	"log/slog"
	"net/http"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/coder/websocket"
	"github.com/google/uuid"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/sprite"
	"github.com/tik-choco/tc-npc/internal/vrm"
)

const (
	suspendQueueCap   = 100
	maxWSMessageBytes = 64 << 20 // Tungstenite's logical message limit.
)

func (s *appState) resolveAvatar(c *character.Character, defaultFile string) *character.Avatar {
	var avatar *character.Avatar
	if c != nil {
		avatar = c.Avatar
	}
	if avatar == nil {
		file := strings.TrimSpace(defaultFile)
		if file == "" {
			return nil
		}
		fallback := character.NewVRMAvatar(file)
		avatar = &fallback
	}
	// Unknown avatar kinds use the VRM library so newer sheets can still
	// degrade gracefully. An explicit missing assignment takes precedence.
	exists := vrm.VRMExists
	if avatar.Kind == "sprite" {
		exists = sprite.SpriteExists
	}
	if found, err := exists(s.mc.DataDir, avatar.File); err == nil && found {
		return avatar
	}
	return nil
}

func (s *appState) avatarRef() *character.Avatar {
	cfg := s.current.Load()
	return s.resolveAvatar(activeCharacter(s, cfg.Character.ActiveID), cfg.Character.AvatarFile)
}

func (s *appState) broadcastAvatar() {
	s.hub.broadcast(object{"type": "avatar", "avatar": s.avatarRef()})
}

func (s *appState) buildHello() helloFrame {
	cfg := s.current.Load()
	c := activeCharacter(s, cfg.Character.ActiveID)
	var ref *characterRef
	if c != nil {
		ref = &characterRef{ID: c.ID, Name: c.Sheet.Name}
	}
	return helloFrame{
		Type: "hello", Version: "1", Modules: flagsFor(cfg), Character: ref,
		Avatar: s.resolveAvatar(c, cfg.Character.AvatarFile),
	}
}

func (s *appState) wsHandler(w http.ResponseWriter, r *http.Request) {
	// Match Axum's WebSocket extractor rejections before passing a
	// well-formed handshake to coder/websocket.
	switch {
	case r.Method != http.MethodGet:
		writeText(w, http.StatusMethodNotAllowed, "Request method must be `GET`")
		return
	case !strings.Contains(strings.ToLower(r.Header.Get("Connection")), "upgrade"):
		writeText(w, http.StatusBadRequest, "Connection header did not include 'upgrade'")
		return
	case !strings.EqualFold(r.Header.Get("Upgrade"), "websocket"):
		writeText(w, http.StatusBadRequest, "`Upgrade` header did not include 'websocket'")
		return
	case r.Header.Get("Sec-WebSocket-Version") != "13":
		writeText(w, http.StatusBadRequest, "`Sec-WebSocket-Version` header did not include '13'")
		return
	case len(r.Header.Values("Sec-WebSocket-Key")) == 0:
		writeText(w, http.StatusBadRequest, "`Sec-WebSocket-Key` header missing")
		return
	case !r.ProtoAtLeast(1, 1):
		writeText(w, http.StatusUpgradeRequired, "WebSocket request couldn't be upgraded since no upgrade state was present")
		return
	}
	if _, ok := w.(http.Hijacker); !ok {
		writeText(w, http.StatusUpgradeRequired, "WebSocket request couldn't be upgraded since no upgrade state was present")
		return
	}

	s.sockets.Add(1)
	defer s.sockets.Done()
	// Axum accepts upgrades from any Origin. HTTP CORS is independent
	// and the Rust router adds no Access-Control-* headers.
	conn, err := websocket.Accept(w, r, &websocket.AcceptOptions{InsecureSkipVerify: true})
	if err != nil {
		return
	}
	s.handleSocket(conn)
}

func (s *appState) handleSocket(conn *websocket.Conn) {
	conn.SetReadLimit(maxWSMessageBytes)
	ctx, cancel := context.WithCancel(context.Background())
	stop := context.AfterFunc(s.ctx, func() { _ = conn.Close(websocket.StatusNoStatusRcvd, "") })
	defer stop()

	s.inputMu.Lock()
	id, client := s.hub.register(s.buildHello(), object{"type": "voice", "active": s.voiceActive.Load()})
	s.inputMu.Unlock()
	written := make(chan struct{})
	go func() {
		defer close(written)
		defer cancel()
		for {
			select {
			case <-ctx.Done():
				return
			case <-client.wake:
				for _, frame := range s.hub.take(client) {
					if err := conn.Write(ctx, websocket.MessageText, frame); err != nil {
						return
					}
				}
			}
		}
	}()
	defer func() {
		s.hub.unregister(id)
		s.inputMu.Lock()
		if s.gate.owner != nil && *s.gate.owner == id {
			slog.Info("npc-server: the client that suspended disconnected, auto-resuming", "client_id", id)
			s.resume(nil)
		}
		s.inputMu.Unlock()

		cancel()
		_ = conn.CloseNow()
		<-written
	}()

	for {
		kind, data, err := conn.Read(ctx)
		if err != nil {
			slog.Debug("npc-server: ws read ended", "error", err)
			return
		}
		if kind != websocket.MessageText {
			continue
		}
		if !utf8.Valid(data) {
			_ = conn.Close(websocket.StatusInvalidFramePayloadData, "")
			return
		}
		s.handleClientMessage(id, data)
	}
}

type queuedRequest struct {
	clientID uint64
	message  any
}

// A pause applies to input and events from every connection. The most
// recent suspender owns the pause, so unrelated disconnects leave it alone.
type suspendGate struct {
	owner *uint64
	queue []queuedRequest
}

func (s *appState) handleClientMessage(id uint64, data []byte) {
	message, err := parseClientMessage(data)
	if err != nil {
		s.hub.sendTo(id, object{"type": "error", "message": "invalid frame: " + err.Error()})
		return
	}
	s.inputMu.Lock()
	defer s.inputMu.Unlock()
	switch msg := message.(type) {
	case clientInput, clientEvent:
		req := queuedRequest{clientID: id, message: msg}
		if s.gate.owner == nil {
			s.processRequest(req)
			return
		}
		if len(s.gate.queue) == suspendQueueCap {
			slog.Warn("npc-server: suspended input/event queue full, dropping oldest queued frame", "cap", suspendQueueCap)
			copy(s.gate.queue, s.gate.queue[1:])
			s.gate.queue = s.gate.queue[:suspendQueueCap-1]
		}
		s.gate.queue = append(s.gate.queue, req)
	case clientCommand:
		s.mc.Bus.Publish(bus.TopicAction, "command", object{"text": msg.Text})
	case clientSignal:
		switch msg {
		case "interrupt":
			// Interrupt still cuts off current playback while suspended.
			s.mc.Bus.Publish(bus.TopicInterrupt, bus.MsgInterrupt, object{})
			s.hub.sendTo(id, object{"type": "interruptAccepted"})
		case "suspend":
			s.gate.owner = &id
			s.hub.sendTo(id, object{"type": "suspendAccepted"})
			s.mc.Bus.Publish(bus.TopicInterrupt, bus.MsgSuspend, object{})
		case "resume":
			s.resume(&id)
		case "voiceStart":
			s.setVoiceActive(true)
		case "voiceStop":
			s.setVoiceActive(false)
		}
	}
}

// Called under inputMu to order the ack and replay before any new input.
func (s *appState) resume(ackTo *uint64) {
	s.gate.owner = nil
	queue := s.gate.queue
	s.gate.queue = nil
	if ackTo != nil {
		s.hub.sendTo(*ackTo, object{"type": "resumeAccepted"})
	}
	s.mc.Bus.Publish(bus.TopicInterrupt, bus.MsgResume, object{})
	for _, req := range queue {
		s.processRequest(req)
	}
}

func (s *appState) processRequest(req queuedRequest) {
	requestID := uuid.NewString()
	var text, speaker string
	var event bool
	switch msg := req.message.(type) {
	case clientInput:
		s.hub.sendTo(req.clientID, object{"type": "inputAccepted", "requestId": requestID})
		text, speaker = msg.Text, valueOr(msg.Speaker, "")
	case clientEvent:
		s.hub.sendTo(req.clientID, object{"type": "eventAccepted", "kind": msg.Kind})
		text, speaker, event = buildEventInstruction(msg), valueOr(msg.UserName, ""), true
	}
	s.echo.mark(text)
	s.hub.broadcast(chatFrame{Type: "chat", Role: "user", Text: text, TS: nowMS()})
	// Stream handles can opt into npc-talk's katakana transliteration.
	s.mc.Bus.Publish(bus.TopicSense, bus.MsgSpeech, bus.SpeechPayload(text, speaker, requestID, event))
}

func (s *appState) setVoiceActive(active bool) {
	s.voiceActive.Store(active)
	command := bus.MsgVoiceStop
	if active {
		command = bus.MsgVoiceStart
	}
	s.mc.Bus.Publish(bus.TopicInterrupt, command, object{})
	// Broadcast even if another tab already set this position.
	s.hub.broadcast(object{"type": "voice", "active": active})
}

func trimOr(value *string, fallback string) string {
	if text := strings.TrimSpace(valueOr(value, "")); text != "" {
		return text
	}
	return fallback
}

func buildEventInstruction(e clientEvent) string {
	user := trimOr(e.UserName, "誰か")
	tier := trimOr(e.Tier, "不明")
	amount := "?"
	if e.Amount != nil {
		amount = strconv.FormatFloat(*e.Amount, 'f', -1, 64)
	}
	comment := func(label string) string {
		if text := trimOr(e.Message, ""); text != "" {
			return fmt.Sprintf("%s:「%s」", label, text)
		}
		return ""
	}

	var detail string
	switch e.Kind {
	case "follow":
		detail = user + "さんが新しくフォローしてくれました。"
	case "subscribe", "sub":
		detail = fmt.Sprintf("%sさんが新しくサブスク（%s）してくれました。", user, tier)
	case "resub":
		detail = fmt.Sprintf("%sさんがサブスクを更新してくれました（継続%sヶ月・%s）。%s", user, amount, tier, comment("コメント"))
	case "gift":
		detail = fmt.Sprintf("%sさんがサブスク（%s）を %s 件ギフトしてくれました。", user, tier, amount)
	case "cheer":
		detail = fmt.Sprintf("%sさんが %s bits を投げてくれました。%s", user, amount, comment("コメント"))
	case "raid":
		detail = fmt.Sprintf("%sさんが %s 人を連れてレイドしてくれました。", user, amount)
	case "points":
		detail = fmt.Sprintf("%sさんがチャンネルポイント「%s」を交換しました。%s", user, trimOr(e.RewardTitle, "不明"), comment("入力"))
	case "donate", "superchat":
		if e.Amount != nil && *e.Amount > 0 {
			detail = user + "さんから" + amount + "円の投げ銭がありました。"
		} else {
			detail = user + "さんから投げ銭がありました。"
		}
	default:
		detail = e.Kind + "というイベントが発生しました"
		if text := trimOr(e.Text, ""); text != "" {
			detail += ": " + text
		}
		detail += "。"
	}

	// Framing the reaction as stage direction keeps the model from
	// reading the event description back verbatim.
	return "【配信イベント】\n" + detail + "\n" +
		"あなたは配信キャラクターです。視聴者に向けて、短く自然なリアクション（一言〜二言）を返してください。イベントの説明文をそのまま読み上げず、キャラクターとして反応してください。"
}
