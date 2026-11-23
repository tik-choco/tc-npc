package mist

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/module"
)

func TestNodeIDPersistsAcrossCalls(t *testing.T) {
	dir := t.TempDir()
	first, err := loadOrCreateNodeID(dir)
	if err != nil {
		t.Fatal(err)
	}
	second, err := loadOrCreateNodeID(dir)
	if err != nil {
		t.Fatal(err)
	}
	if second != first || !strings.HasPrefix(first, "tc-npc-") || len(first) != len("tc-npc-")+8 {
		t.Fatalf("unexpected node IDs %q and %q", first, second)
	}
}

type testBroadcast struct {
	room       string
	data       []byte
	unreliable bool
}

type testEngine struct {
	events *engineEvents
	joined chan string
	sends  chan testBroadcast
	left   bool
}

func (e *testEngine) start(string, string) (*engineEvents, error) { return e.events, nil }
func (e *testEngine) joinRoom(room string) {
	e.joined <- room
	e.events.push(&roomEvent{kind: eventRoomJoined, room: room})
}
func (e *testEngine) broadcast(room string, data []byte, unreliable bool) {
	e.sends <- testBroadcast{room, data, unreliable}
}
func (e *testEngine) storageGet(_ context.Context, cid string) ([]byte, error) {
	if cid != "test-cid" {
		return nil, fmt.Errorf("unexpected cid %q", cid)
	}
	return []byte(`{"app":"tc-town","version":1,"kind":"catalog-character","character":{
		"id":"town-char","createdAt":"created","updatedAt":"updated","voiceModel":"voice",
		"voiceName":null,"avatar":{"kind":"vrm"},"sheet":{"name":"Town NPC","speechStyle":"calm"}
	}}`), nil
}
func (e *testEngine) leaveAllRooms() { e.left = true }

func TestModuleRoutesRoomsAndLeavesAfterBye(t *testing.T) {
	ctx, cancel := context.WithTimeout(t.Context(), 5*time.Second)
	defer cancel()
	cfg := config.DefaultConfig()
	cfg.Mist.Enabled = true
	cfg.Mist.RoomID = " presence "
	cfg.Mist.AvatarLink.Enabled = true
	cfg.Mist.AvatarLink.Name = " Example "
	mc := module.Ctx{Bus: bus.New(), Config: &cfg, DataDir: t.TempDir()}
	sub := mc.Bus.Subscribe()
	defer sub.Close()
	e := &testEngine{events: newEngineEvents(), joined: make(chan string, 3), sends: make(chan testBroadcast, 8)}
	m := &mistModule{engine: e}
	done := make(chan error, 1)
	go func() { done <- m.Run(ctx, mc) }()
	for _, room := range []string{catalogRoomID, "presence", cfg.Mist.AvatarLink.RoomID} {
		if got := receiveTestValue(t, e.joined); got != room {
			t.Fatalf("joined %q, want %q", got, room)
		}
	}
	checkMessage := func(kind string, unreliable bool) bus.Message {
		t.Helper()
		sent := receiveTestValue(t, e.sends)
		message := bus.Message{Payload: json.RawMessage(sent.data)}
		if sent.room != cfg.Mist.AvatarLink.RoomID || message.Str("t") != kind || sent.unreliable != unreliable {
			t.Fatalf("broadcast = %s in %q, unreliable=%v, want %q", sent.data, sent.room, sent.unreliable, kind)
		}
		return message
	}
	if name := checkMessage("hello", false).Str("name"); name != "Example" {
		t.Fatalf("hello name = %q", name)
	}
	e.events.push(&roomEvent{kind: eventRaw, room: catalogRoomID, data: []byte(`{"type":"tc-town:catalog-entry","cid":"test-cid"}`)})
	if discovery := receiveTestValue(t, sub.C); discovery.Topic != bus.TopicUI || discovery.Type != "action_log" ||
		discovery.Str("text") != "mist: キャラクター「Town NPC」を発見 (catalog)" {
		t.Fatalf("unexpected catalog discovery: %+v", discovery)
	}
	data, err := os.ReadFile(filepath.Join(mc.DataDir, "catalog", "town-char.json"))
	if err != nil {
		t.Fatal(err)
	}
	var stored character.Character
	if err := json.Unmarshal(data, &stored); err != nil || stored.ID != "town-char" || stored.Avatar != nil ||
		stored.CreatedAt != "created" || stored.UpdatedAt != "updated" || stored.Sheet.SpeechStyle != "calm" ||
		stored.VoiceModel == nil || *stored.VoiceModel != "voice" || stored.VoiceName != nil {
		t.Fatalf("unexpected stored character: %s, %v", data, err)
	}
	if _, err := os.Stat(filepath.Join(mc.DataDir, "characters")); !os.IsNotExist(err) {
		t.Fatalf("discovery touched active character store: %v", err)
	}
	mc.Bus.Publish(bus.TopicUI, "speaking", map[string]bool{"active": true})
	mc.Bus.Publish(bus.TopicUI, "speaking_level", map[string]float64{"level": 0.5})
	checkMessage("speaking", false)
	checkMessage("mouth", true)
	for _, kind := range []string{bus.MsgInterrupt, bus.MsgSuspend, bus.MsgVoiceStop} {
		mc.Bus.Publish(bus.TopicInterrupt, kind, nil)
		checkMessage("stop", false)
	}
	cancel()
	checkMessage("bye", false)
	if err := receiveTestValue(t, done); err != nil || !e.left {
		t.Fatalf("module shutdown = %v, left rooms = %v", err, e.left)
	}
}

func TestCatalogRejectsPathIDs(t *testing.T) {
	for _, id := range []string{"../characters/active", `..\characters\active`, `C:active`} {
		if err := saveDiscovered(t.TempDir(), &character.Character{ID: id}); err == nil {
			t.Errorf("saved catalog entry with path ID %q", id)
		}
	}
}

func receiveTestValue[T any](t *testing.T, ch <-chan T) T {
	t.Helper()
	select {
	case value := <-ch:
		return value
	case <-time.After(5 * time.Second):
		t.Fatal("timed out waiting for mist event")
		var zero T
		return zero
	}
}
