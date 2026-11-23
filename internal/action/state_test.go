package action

import (
	"encoding/json"
	"testing"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
)

func location(name string) config.LocationConfig {
	return config.LocationConfig{Name: name, X: 1, Y: 2, Heading: 90}
}

func TestMapDataFromConfigClonesLocationsAndRoutes(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.Action.Locations = []config.LocationConfig{location("Home")}
	cfg.Action.Routes = []config.RouteConfig{{
		Name: "Patrol", Loop: true, Waypoints: []config.WaypointConfig{{Location: "Home"}},
	}}
	data := mapDataFromConfig(&cfg)
	cfg.Action.Locations[0].Name = "Elsewhere"
	cfg.Action.Routes[0].Name = "Elsewhere"
	cfg.Action.Routes[0].Waypoints[0].Location = "Elsewhere"
	if len(data.locations) != 1 || data.locations[0].Name != "Home" {
		t.Fatalf("locations = %+v", data.locations)
	}
	if len(data.routes) != 1 || data.routes[0].Name != "Patrol" || data.routes[0].Waypoints[0].Location != "Home" {
		t.Fatalf("routes = %+v", data.routes)
	}
}

func TestMapReloadReplacesPreviousContents(t *testing.T) {
	first := config.DefaultConfig()
	first.Action.Locations = []config.LocationConfig{location("Home")}
	state := actionState{data: mapDataFromConfig(&first)}
	previous := state.mapSnapshot()
	if previous.locations[0].Name != "Home" {
		t.Fatalf("initial locations = %+v", previous.locations)
	}

	second := config.DefaultConfig()
	second.Action.Locations = []config.LocationConfig{location("Shrine"), location("Cafe")}
	payload, err := json.Marshal(second)
	if err != nil {
		t.Fatal(err)
	}
	state.handleConfigUpdated(bus.Message{Topic: bus.TopicConfig, Type: bus.MsgConfigUpdated, Payload: payload})
	reloaded := state.mapSnapshot()
	if len(reloaded.locations) != 2 || reloaded.locations[0].Name != "Shrine" || reloaded.locations[1].Name != "Cafe" {
		t.Fatalf("reloaded locations = %+v", reloaded.locations)
	}
	if len(previous.locations) != 1 || previous.locations[0].Name != "Home" {
		t.Fatalf("original snapshot changed: %+v", previous.locations)
	}

	state.handleConfigUpdated(bus.Message{Payload: json.RawMessage(`{"action":{"locations":[{}]}}`)})
	if got := state.mapSnapshot(); len(got.locations) != 2 || got.locations[0].Name != "Shrine" {
		t.Fatalf("invalid reload replaced locations: %+v", got.locations)
	}
}

func TestMapLookupsAreCaseInsensitive(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.Action.Locations = []config.LocationConfig{location("Home")}
	cfg.Action.Routes = []config.RouteConfig{{Name: "Patrol"}}
	data := mapDataFromConfig(&cfg)
	for _, name := range []string{"home", "HOME"} {
		if _, ok := findLocation(data.locations, name); !ok {
			t.Errorf("missing location %q", name)
		}
	}
	if _, ok := findLocation(data.locations, "nowhere"); ok {
		t.Error("found nonexistent location")
	}
	for _, name := range []string{"PATROL", "patrol"} {
		if _, ok := data.findRoute(name); !ok {
			t.Errorf("missing route %q", name)
		}
	}
	if _, ok := data.findRoute("nope"); ok {
		t.Error("found nonexistent route")
	}
}
