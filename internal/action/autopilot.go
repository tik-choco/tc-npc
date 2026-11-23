package action

import (
	"context"
	"fmt"
	"math"
	"time"

	"github.com/tik-choco/tc-npc/internal/config"
)

const (
	minDistanceThreshold = 0.01
	minRotationThreshold = 0.5
	minMoveTime          = 0.1
	// The waypoint wait flag requests a fixed two-second pause.
	waypointWait = 2 * time.Second
)

type autopilot struct {
	nav *navigator
}

func shortestRotation(target, current float64) float64 {
	rotation := target - current
	if rotation > 180 {
		rotation -= 360
	}
	if rotation < -180 {
		rotation += 360
	}
	return rotation
}

// Face the target, then use the remainder of the time budget for translation.
func (a autopilot) goTo(ctx context.Context, x, y, seconds float64) (bool, error) {
	current := a.nav.pos.get()
	dx, dy := x-current.X, y-current.Y
	distance := math.Sqrt(dx*dx + dy*dy)
	if distance < minDistanceThreshold {
		return true, nil
	}

	targetHeading := math.Atan2(dx, dy) * (180 / math.Pi)
	if targetHeading < 0 {
		targetHeading += 360
	}
	rotation := shortestRotation(targetHeading, current.Heading)
	moveTime := max(seconds-math.Abs(rotation)/maxDPS(), minMoveTime)
	if math.Abs(rotation) > minRotationThreshold {
		completed, err := a.nav.turnDegrees(ctx, rotation)
		if err != nil || !completed {
			return completed, err
		}
	}
	return a.nav.moveMetersInTime(ctx, distance, moveTime)
}

func (a autopilot) goToWithHeading(ctx context.Context, x, y, heading, seconds float64) (bool, error) {
	completed, err := a.goTo(ctx, x, y, seconds)
	if err != nil || !completed {
		return completed, err
	}
	rotation := shortestRotation(heading, a.nav.pos.get().Heading)
	if math.Abs(rotation) > minRotationThreshold {
		return a.nav.turnDegrees(ctx, rotation)
	}
	return true, nil
}

func (a autopilot) returnToOrigin(ctx context.Context, seconds float64) (bool, error) {
	return a.goTo(ctx, 0, 0, seconds)
}

func (a autopilot) runRoute(ctx context.Context, route config.RouteConfig, locations []config.LocationConfig, log func(string)) error {
	log(fmt.Sprintf("Starting route '%s' (loop=%t, %d waypoints)", route.Name, route.Loop, len(route.Waypoints)))

	// An empty loop has no work to repeat.
	for lap := uint32(1); len(route.Waypoints) > 0; lap++ {
		if route.Loop {
			log(fmt.Sprintf("--- Lap %d ---", lap))
		}
		for i, waypoint := range route.Waypoints {
			if ctx.Err() != nil {
				log("Route stopped")
				return nil
			}
			loc, ok := findLocation(locations, waypoint.Location)
			if !ok {
				return fmt.Errorf("location '%s' not found", waypoint.Location)
			}

			log(fmt.Sprintf("Waypoint %d/%d: '%s'", i+1, len(route.Waypoints), loc.Name))
			completed, err := a.goToWithHeading(ctx, loc.X, loc.Y, loc.Heading, waypoint.Seconds)
			if err != nil {
				return err
			}
			if !completed {
				log("Route stopped")
				return nil
			}
			log(fmt.Sprintf("Arrived at '%s' - %s", loc.Name, a.nav.pos.get()))

			if waypoint.Wait {
				log(fmt.Sprintf("Waiting %.1fs...", waypointWait.Seconds()))
				completed, _ := sleepCancellable(ctx, waypointWait)
				if !completed {
					log("Route stopped")
					return nil
				}
			}
		}
		if !route.Loop {
			break
		}
	}

	log(fmt.Sprintf("Route '%s' completed", route.Name))
	return nil
}
