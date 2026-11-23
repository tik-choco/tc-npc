// Package mist integrates mistlib rooms for tc-town catalog discovery,
// presence, and the Avatar Link brain role.
package mist

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/module"
)

// New locates and loads the mistlib DLL. The engine starts in Run.
func New(mc module.Ctx) (module.Module, error) {
	e, err := loadEngine(mc.DataDir)
	if err != nil {
		return nil, err
	}
	return &mistModule{engine: e}, nil
}

type mistModule struct{ engine engine }

func (*mistModule) Name() string { return "npc-mist" }

func (m *mistModule) Run(ctx context.Context, mc module.Ctx) error {
	if !mc.Config.Mist.Enabled {
		slog.Debug("npc-mist: mist.enabled is false, idling until shutdown")
		<-ctx.Done()
		return nil
	}
	if ctx.Err() != nil {
		return nil
	}
	nodeID, err := loadOrCreateNodeID(mc.DataDir)
	if err != nil {
		return err
	}
	rooms := []string{catalogRoomID}
	if room := strings.TrimSpace(mc.Config.Mist.RoomID); room != "" && !slices.Contains(rooms, room) {
		rooms = append(rooms, room)
	}
	var (
		avatar   *avatarBridge
		sub      *bus.Subscription
		messages <-chan bus.Message
	)
	if mc.Config.Mist.AvatarLink.Enabled {
		avatar = newAvatarBridge(mc, nodeID)
		if avatar.roomID == "" {
			return errors.New("mist.avatar_link.room_id must not be empty")
		}
		if !slices.Contains(rooms, avatar.roomID) {
			rooms = append(rooms, avatar.roomID)
		}
		sub = mc.Bus.Subscribe()
		defer sub.Close()
		messages = sub.C
	}

	slog.Info("npc-mist: starting mist engine", "node_id", nodeID, "rooms", rooms)
	events, err := m.engine.start(nodeID, mc.Config.Mist.SignalingURL)
	if err != nil {
		return fmt.Errorf("mist: initializing mistlib engine: %w", err)
	}
	defer func() {
		if avatar != nil && avatar.joined && avatar.send(m.engine, brainMessage{Kind: "bye"}) {
			// Give the queued broadcast time to reach the send worker before
			// tearing down the sessions. The DLL cannot report enqueue status.
			time.Sleep(100 * time.Millisecond)
		}
		slog.Info("npc-mist: leaving rooms")
		m.engine.leaveAllRooms()
	}()
	// Storage calls in the DLL block native threads until they finish. Limit
	// concurrent calls while keeping room events and Avatar Link responsive.
	catalogCtx, cancelCatalog := context.WithCancel(ctx)
	defer cancelCatalog()
	catalogSlots := make(chan struct{}, 8)
	for _, room := range rooms {
		m.engine.joinRoom(room)
	}

	var failedRooms []string
	var dropped uint64
	heartbeat := time.NewTimer(helloInterval)
	defer heartbeat.Stop()
	for ctx.Err() == nil {
		select {
		case <-ctx.Done():
			return nil
		case <-heartbeat.C:
			for _, room := range failedRooms {
				m.engine.joinRoom(room)
			}
			failedRooms = failedRooms[:0]
			if avatar != nil && avatar.joined {
				avatar.hello(m.engine, time.Now())
			}
			heartbeat.Reset(helloInterval)
		case <-events.ready:
			event := events.pop()
			if event == nil {
				continue
			}
			avatarRoom := avatar != nil && avatar.roomID == event.room
			switch event.kind {
			case eventRoomJoined:
				slog.Info("npc-mist: joined room", "room", event.room)
				if avatarRoom {
					avatar.joined = true
					avatar.hello(m.engine, time.Now())
					heartbeat.Reset(helloInterval)
				}
			case eventRoomJoinFailed:
				slog.Warn("npc-mist: failed to join room, will retry", "room", event.room, "error", string(event.data))
				failedRooms = append(failedRooms, event.room)
			case eventRaw:
				now := time.Now()
				if avatarRoom && avatar.joined && avatar.receive(mc.Bus, event.data, now) {
					avatar.hello(m.engine, now)
				}
				if event.room == catalogRoomID {
					if cid, ok := catalogCID(event.data); ok {
						go fetchCatalog(catalogCtx, catalogSlots, m.engine, mc, cid)
					}
				}
			}
		case entry := <-events.logs:
			slog.Log(ctx, entry.level, entry.message, "component", "mistlib")
		case event, ok := <-messages:
			if !ok {
				return nil
			}
			if total := sub.Dropped(); total != dropped {
				slog.Warn("npc-mist: bus receiver lagged", "skipped", total-dropped)
				dropped = total
			}
			if message := avatar.fromBus(event, time.Now()); message.Kind != "" {
				avatar.send(m.engine, message)
			}
		}
	}
	return nil
}

var nodeIDMu sync.Mutex

func loadOrCreateNodeID(dataDir string) (string, error) {
	nodeIDMu.Lock()
	defer nodeIDMu.Unlock()
	path := filepath.Join(dataDir, "mist-node-id.txt")
	if existing, err := os.ReadFile(path); err == nil && utf8.Valid(existing) {
		if id := strings.TrimSpace(string(existing)); id != "" {
			return id, nil
		}
	}
	nodeID := "tc-npc-" + uuid.NewString()[:8]
	if err := os.MkdirAll(dataDir, 0o755); err != nil {
		return "", fmt.Errorf("failed to create %s: %w", dataDir, err)
	}
	if err := writeAtomic(path, []byte(nodeID)); err != nil {
		return "", fmt.Errorf("failed to write %s: %w", path, err)
	}
	return nodeID, nil
}

func writeAtomic(path string, data []byte) error {
	file, err := os.CreateTemp(filepath.Dir(path), ".mist-*")
	if err != nil {
		return err
	}
	defer os.Remove(file.Name())
	_, writeErr := file.Write(data)
	if err := errors.Join(writeErr, file.Close()); err != nil {
		return err
	}
	return os.Rename(file.Name(), path)
}
