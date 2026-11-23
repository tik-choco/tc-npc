package action

import (
	"context"
	"fmt"
	"math"
	"strconv"
	"strings"
	"unicode"
)

const calibrationUnavailable = "キャリブレーションコマンド(cal/mcal)はこのRust移植版では利用できません。速度モデル定数は固定値を使用しています。"

// dispatchCommand reports whether the input was handled as a CLI command.
func (s *actionState) dispatchCommand(text string) bool {
	parts := strings.Fields(text)
	if len(parts) == 0 {
		return true
	}
	name := lower(parts[0])
	if name == "stop" {
		s.activeMu.Lock()
		defer s.activeMu.Unlock()
		s.stopActiveLocked()
		s.runImmediate(name)
		return true
	}
	if s.runImmediate(name) {
		return true
	}
	switch name {
	case "w", "s", "a", "d", "fw", "tr", "tl", "jump", "mv", "mb", "rt", "lt", "mvt", "rtt", "ltt", "goto", "home", "go", "route":
		s.executeActions([]actionItem{{Name: name, Args: parts[1:]}})
		return true
	default:
		return false
	}
}

func (s *actionState) runImmediate(name string) bool {
	switch name {
	case "stop":
		// A stop inside an action sequence zeros axes without joining its own task.
		_ = s.body.Vertical(0)
		_ = s.body.Horizontal(0)
		_ = s.body.LookHorizontal(0)
		s.log("Stopping movements...")
	case "pos":
		s.logPosition()
	case "reset":
		s.navigator.pos.reset()
		s.navigator.publishPosition()
		s.log("Position reset to origin")
	case "locations":
		s.log(formatLocations(s.mapSnapshot()))
	case "routes":
		s.log(formatRoutes(s.mapSnapshot()))
	case "cal", "mcal":
		s.log(calibrationUnavailable)
	default:
		return false
	}
	return true
}

func (s *actionState) logPosition() {
	s.log("Position: " + s.navigator.pos.get().String())
}

func formatLocations(data mapData) string {
	if len(data.locations) == 0 {
		return "No locations configured"
	}
	var out strings.Builder
	fmt.Fprintf(&out, "%-15s %-8s %-8s %-10s\n", "Name", "X", "Y", "Heading")
	out.WriteString("-------------------------------------------")
	for _, loc := range data.locations {
		fmt.Fprintf(&out, "\n%-15s %-8.1f %-8.1f %-10.1f°", loc.Name, loc.X, loc.Y, loc.Heading)
	}
	return out.String()
}

func formatRoutes(data mapData) string {
	if len(data.routes) == 0 {
		return "No routes configured"
	}
	lines := make([]string, 0, len(data.routes))
	for _, route := range data.routes {
		lines = append(lines, fmt.Sprintf("%-15s %s", route.Name, routePath(route)))
	}
	return strings.Join(lines, "\n")
}

func (s *actionState) stopActiveTask() {
	s.activeMu.Lock()
	defer s.activeMu.Unlock()
	s.stopActiveLocked()
}

func (s *actionState) stopActiveLocked() {
	if task := s.active; task != nil {
		task.cancel()
		<-task.done
		s.active = nil
		s.log("Background task stopped")
	}
}

func (s *actionState) executeActions(items []actionItem) {
	s.activeMu.Lock()
	defer s.activeMu.Unlock()
	if s.shutdown.Err() != nil {
		return
	}
	s.stopActiveLocked()
	ctx, cancel := context.WithCancel(s.shutdown)
	task := &activeTask{cancel: cancel, done: make(chan struct{})}
	s.active = task
	go func() {
		defer close(task.done)
		defer cancel()
		for _, item := range items {
			if ctx.Err() != nil {
				s.log("[Action] Sequence cancelled")
				return
			}
			s.log(fmt.Sprintf("[Action] Executing: %s %s", item.Name, debugArgs(item.Args)))
			name := lower(item.Name)
			if err := s.runCommand(ctx, name, item.Args); err != nil {
				s.log(fmt.Sprintf("Error executing '%s': %v", name, err))
			}
		}
		s.log("[Action] Task sequence completed")
	}()
}

func (s *actionState) runCommand(ctx context.Context, name string, args []string) error {
	var err error
	switch name {
	case "w":
		err = s.body.Vertical(maxAxisValue)
	case "s":
		err = s.body.Vertical(minAxisValue)
	case "a":
		err = s.body.Horizontal(minAxisValue)
	case "d":
		err = s.body.Horizontal(maxAxisValue)
	case "fw":
		_, err = s.controller.moveForward(ctx, parseFloat(args, 0, 2))
	case "tr":
		_, err = s.controller.turn(ctx, maxAxisValue, parseFloat(args, 0, 1))
	case "tl":
		_, err = s.controller.turn(ctx, minAxisValue, parseFloat(args, 0, 1))
	case "jump":
		_, err = s.controller.jump(ctx)
	case "mv":
		_, err = s.navigator.moveMeters(ctx, parseFloat(args, 0, 1))
	case "mb":
		_, err = s.navigator.moveMeters(ctx, -parseFloat(args, 0, 1))
	case "rt":
		_, err = s.navigator.turnDegrees(ctx, parseFloat(args, 0, 90))
	case "lt":
		_, err = s.navigator.turnDegrees(ctx, -parseFloat(args, 0, 90))
	case "mvt":
		_, err = s.navigator.moveMetersInTime(ctx, parseFloat(args, 0, 2), parseFloat(args, 1, 4))
	case "rtt":
		_, err = s.navigator.turnDegreesInTime(ctx, parseFloat(args, 0, 180), parseFloat(args, 1, 3))
	case "ltt":
		_, err = s.navigator.turnDegreesInTime(ctx, -parseFloat(args, 0, 180), parseFloat(args, 1, 3))
	case "goto":
		if len(args) < 2 {
			s.log("Usage: goto <x> <y> [heading] [seconds]")
			return nil
		}
		x, y, seconds := parseFloat(args, 0, 0), parseFloat(args, 1, 0), parseFloat(args, 3, 10)
		if len(args) >= 3 {
			_, err = s.autopilot.goToWithHeading(ctx, x, y, parseFloat(args, 2, 0), seconds)
		} else {
			_, err = s.autopilot.goTo(ctx, x, y, seconds)
		}
		if err == nil {
			s.logPosition()
		}
	case "home":
		_, err = s.autopilot.returnToOrigin(ctx, parseFloat(args, 0, 10))
		if err == nil {
			s.logPosition()
		}
	case "go":
		if len(args) == 0 {
			s.log("Usage: go <location_name> [seconds]")
			return nil
		}
		data := s.mapSnapshot()
		loc, ok := findLocation(data.locations, args[0])
		if !ok {
			s.log(fmt.Sprintf("Location '%s' not found. Use 'locations' to list.", args[0]))
			return nil
		}
		_, err = s.autopilot.goToWithHeading(ctx, loc.X, loc.Y, loc.Heading, parseFloat(args, 1, 10))
		if err == nil {
			s.logPosition()
		}
	case "route":
		if len(args) == 0 {
			s.log("Usage: route <route_name>")
			return nil
		}
		data := s.mapSnapshot()
		route, ok := data.findRoute(args[0])
		if !ok {
			s.log(fmt.Sprintf("Route '%s' not found. Use 'routes' to list.", args[0]))
			return nil
		}
		if err := s.autopilot.runRoute(ctx, route, data.locations, s.log); err != nil {
			s.log(fmt.Sprintf("[Route] Error: %v", err))
		}
	default:
		if !s.runImmediate(name) {
			s.log(fmt.Sprintf("Unknown action '%s'", name))
		}
	}
	return err
}

func parseFloat(args []string, index int, fallback float64) float64 {
	if index >= len(args) || strings.ContainsAny(args[index], "_xX") {
		return fallback
	}
	value, err := strconv.ParseFloat(args[index], 64)
	if err != nil || math.IsNaN(value) || math.IsInf(value, 0) {
		return fallback
	}
	return value
}

// The action-log argument list uses Rust's Debug string escapes.
func debugArgs(args []string) string {
	var out strings.Builder
	out.WriteByte('[')
	for i, arg := range args {
		if i != 0 {
			out.WriteString(", ")
		}
		out.WriteByte('"')
		for _, r := range arg {
			switch r {
			case 0:
				out.WriteString("\\0")
			case '\n':
				out.WriteString("\\n")
			case '\r':
				out.WriteString("\\r")
			case '\t':
				out.WriteString("\\t")
			case '"', '\\':
				out.WriteByte('\\')
				out.WriteRune(r)
			default:
				if !unicode.IsPrint(r) || unicode.In(r, unicode.Mn, unicode.Me, unicode.Other_Grapheme_Extend) {
					fmt.Fprintf(&out, "\\u{%x}", r)
				} else {
					out.WriteRune(r)
				}
			}
		}
		out.WriteByte('"')
	}
	out.WriteByte(']')
	return out.String()
}
