package action

import (
	"context"
	"slices"
	"strings"
	"sync"
	"sync/atomic"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"golang.org/x/text/cases"
	"golang.org/x/text/language"
)

const msgActionLog = "action_log"

type mapData struct {
	locations []config.LocationConfig
	routes    []config.RouteConfig
}

func mapDataFromConfig(cfg *config.Config) mapData {
	routes := slices.Clone(cfg.Action.Routes)
	for i := range routes {
		routes[i].Waypoints = slices.Clone(routes[i].Waypoints)
	}
	return mapData{locations: slices.Clone(cfg.Action.Locations), routes: routes}
}

func lower(s string) string {
	return cases.Lower(language.Und).String(s)
}

func findLocation(locations []config.LocationConfig, name string) (config.LocationConfig, bool) {
	lowercase := cases.Lower(language.Und)
	name = lowercase.String(name)
	for _, loc := range locations {
		if lowercase.String(loc.Name) == name {
			return loc, true
		}
	}
	return config.LocationConfig{}, false
}

func (m mapData) findRoute(name string) (config.RouteConfig, bool) {
	lowercase := cases.Lower(language.Und)
	name = lowercase.String(name)
	for _, route := range m.routes {
		if lowercase.String(route.Name) == name {
			return route, true
		}
	}
	return config.RouteConfig{}, false
}

func routePath(route config.RouteConfig) string {
	locations := make([]string, 0, len(route.Waypoints))
	for _, wp := range route.Waypoints {
		locations = append(locations, wp.Location)
	}
	path := strings.Join(locations, " -> ")
	if route.Loop {
		path += " [loop]"
	}
	return path
}

type activeTask struct {
	cancel context.CancelFunc
	done   chan struct{}
}

type actionState struct {
	bus        *bus.Bus
	shutdown   context.Context
	body       Actuator
	controller controller
	navigator  *navigator
	autopilot  autopilot

	mapMu sync.RWMutex
	data  mapData

	llm          *llm.Client
	model        string
	staticPrompt string
	historySize  uint32
	history      []llm.ChatMessage // Owned by the request holding chatBusy.
	chatBusy     atomic.Bool

	// Serialize sequence replacement and top-level stop through the old
	// sequence's last input, so its cleanup cannot overwrite a new hold.
	activeMu sync.Mutex
	active   *activeTask
}

func (s *actionState) log(text string) {
	s.bus.Publish(bus.TopicUI, msgActionLog, struct {
		Text string `json:"text"`
	}{text})
}

// Map snapshots are immutable after publication. A running route keeps the
// same locations it started with, even if config changes during the route.
func (s *actionState) mapSnapshot() mapData {
	s.mapMu.RLock()
	defer s.mapMu.RUnlock()
	return s.data
}

func (s *actionState) replaceMap(data mapData) {
	s.mapMu.Lock()
	defer s.mapMu.Unlock()
	s.data = data
}
