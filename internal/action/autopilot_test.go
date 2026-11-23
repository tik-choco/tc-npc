package action

import (
	"context"
	"slices"
	"testing"
	"testing/synctest"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
)

func TestLoopingRouteStopsDuringWaypointWait(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		nav := navigator{body: &RecordingActuator{}, bus: bus.New()}
		pilot := autopilot{nav: &nav}
		route := config.RouteConfig{
			Name: "Patrol", Loop: true, Waypoints: []config.WaypointConfig{{Location: "home", Wait: true}},
		}
		ctx, cancel := context.WithCancel(t.Context())
		defer cancel()
		go func() {
			time.Sleep(500 * time.Millisecond)
			cancel()
		}()
		var logs []string
		if err := pilot.runRoute(ctx, route, []config.LocationConfig{{Name: "Home"}}, func(line string) {
			logs = append(logs, line)
		}); err != nil {
			t.Fatal(err)
		}
		want := []string{
			"Starting route 'Patrol' (loop=true, 1 waypoints)",
			"--- Lap 1 ---",
			"Waypoint 1/1: 'Home'",
			"Arrived at 'Home' - X=0.00 Y=0.00 Heading=0.0°",
			"Waiting 2.0s...",
			"Route stopped",
		}
		if !slices.Equal(logs, want) {
			t.Fatalf("route logs = %q, want %q", logs, want)
		}
	})
}
