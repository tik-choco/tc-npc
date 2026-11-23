package server

import (
	"context"
	"log/slog"
	"net/http"
	"sync"

	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/mistclient"
)

type syncStatus struct {
	Pending    bool     `json:"pending"`
	Applied    bool     `json:"applied"`
	Error      *string  `json:"error"`
	Warnings   []string `json:"warnings"`
	UpdatedAt  *string  `json:"updated_at"`
	Generation uint64   `json:"generation"`
}

type mistSync struct {
	mu     sync.Mutex
	state  syncStatus
	latest *config.Config
	wake   chan struct{}
}

func (m *mistSync) start(ctx context.Context, cfg *config.Config) <-chan struct{} {
	m.wake = make(chan struct{}, 1)
	m.submit(cfg)
	done := make(chan struct{})
	go func() {
		defer close(done)
		var handled uint64
		for ctx.Err() == nil {
			select {
			case <-ctx.Done():
				return
			case <-m.wake:
			}
			m.mu.Lock()
			generation, cfg := m.state.Generation, m.latest
			m.mu.Unlock()
			if generation == handled {
				continue
			}
			handled = generation
			applied, warnings, err := syncMist(ctx, cfg)
			if ctx.Err() != nil {
				return
			}
			m.mu.Lock()
			// Even if a newer save arrived mid-call, its reconciliation
			// must run after this one so it wins at the daemon too.
			if m.state.Generation == generation {
				m.state.Pending = false
				now := utcTimestamp()
				m.state.UpdatedAt = &now
				m.state.Applied = applied
				m.state.Warnings = warnings
				m.state.Error = nil
				if err != nil {
					message := err.Error()
					slog.Warn("mistl external registration failed", "error", message)
					m.state.Error = &message
				}
			}
			m.mu.Unlock()
		}
	}()
	return done
}

func (m *mistSync) submit(cfg *config.Config) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.wake == nil {
		return
	}
	m.state.Generation++
	m.state.Pending = true
	m.latest = cfg
	select {
	case m.wake <- struct{}{}:
	default:
	}
}

func (m *mistSync) status() syncStatus {
	m.mu.Lock()
	defer m.mu.Unlock()
	status := m.state
	if status.Warnings == nil {
		status.Warnings = []string{}
	}
	return status
}

func syncMist(ctx context.Context, cfg *config.Config) (bool, []string, error) {
	payload := mistclient.BuildRegistration(cfg)
	client := mistclient.New(&cfg.Mist)
	if len(payload.Rooms) == 0 {
		_, err := client.Remove(ctx)
		return false, nil, err
	}
	result, err := client.Apply(ctx, &payload)
	return err == nil, result.Warnings, err
}

func (s *appState) apiMistSync(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, s.mist.status())
}

func (s *appState) apiMistRooms(w http.ResponseWriter, r *http.Request) {
	result, err := mistclient.New(&s.current.Load().Mist).Get(r.Context())
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	writeJSON(w, result)
}
