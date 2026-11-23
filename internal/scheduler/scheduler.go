// Package scheduler publishes announcements and actions on the bus at their
// scheduled local times. The speech module handles audio playback.
package scheduler

import (
	"context"
	"log/slog"
	"strconv"
	"strings"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/module"
)

// New creates the scheduler even when disabled so it can receive config updates.
func New(mc module.Ctx) (module.Module, error) {
	return schedulerModule{}, nil
}

type schedulerModule struct{}

func (schedulerModule) Name() string { return "npc-scheduler" }

func (schedulerModule) Run(ctx context.Context, mc module.Ctx) error {
	sub := mc.Bus.Subscribe()
	defer sub.Close()

	current := mc.Config
	entries := computeSchedule(current.Scheduler, time.Now())
	timer := time.NewTimer(0)
	defer timer.Stop()
	wakeAt, tick := scheduleWake(timer, entries)
	var dropped uint64

	for {
		select {
		case <-ctx.Done():
			slog.Info("npc-scheduler: shutting down")
			return nil
		case <-tick:
			// Use the selected wake time so entries at the same time fire
			// together and later entries keep their own scheduled wake.
			for i := range entries {
				entry := &entries[i]
				if !entry.next.After(wakeAt) {
					FireAnnouncement(mc.Bus, &entry.announcement, &current.Scheduler.Defaults)
					entry.next = entry.next.Add(24 * time.Hour)
				}
			}
			wakeAt, tick = scheduleWake(timer, entries)
		case msg, ok := <-sub.C:
			if !ok {
				slog.Info("npc-scheduler: bus closed, shutting down")
				return nil
			}
			if total := sub.Dropped(); total != dropped {
				slog.Warn("npc-scheduler: bus receiver lagged, some messages dropped", "skipped", total-dropped)
				dropped = total
			}
			if msg.Topic != bus.TopicConfig || msg.Type != bus.MsgConfigUpdated {
				continue
			}

			// Profile activation publishes the copied schedule and its active
			// profile ID together. The ID is metadata for the UI.
			var updated config.Config
			if err := msg.Decode(&updated); err != nil {
				slog.Warn("npc-scheduler: failed to deserialize updated config, keeping current schedule", "error", err)
				continue
			}
			current = &updated
			slog.Info("npc-scheduler: config updated, schedule recomputed")
			entries = computeSchedule(current.Scheduler, time.Now())
			wakeAt, tick = scheduleWake(timer, entries)
		}
	}
}

type scheduledAnnouncement struct {
	announcement config.AnnouncementConfig
	next         time.Time
}

// A nil tick leaves the scheduler waiting for shutdown or a config update.
func scheduleWake(timer *time.Timer, entries []scheduledAnnouncement) (time.Time, <-chan time.Time) {
	if len(entries) == 0 {
		timer.Stop()
		return time.Time{}, nil
	}
	wakeAt := entries[0].next
	for _, entry := range entries[1:] {
		if entry.next.Before(wakeAt) {
			wakeAt = entry.next
		}
	}
	timer.Reset(time.Until(wakeAt))
	return wakeAt, timer.C
}

func computeSchedule(cfg config.SchedulerConfig, now time.Time) []scheduledAnnouncement {
	if !cfg.Enabled {
		slog.Info("npc-scheduler: scheduler disabled, idling until config update")
		return nil
	}

	var entries []scheduledAnnouncement
	for _, ann := range cfg.Announcements {
		clockTime, ok := parseTime(ann.Time)
		if !ok {
			slog.Warn("npc-scheduler: invalid announcement time (expected HH:MM or HH:MM:SS), skipping", "time", ann.Time)
			continue
		}
		next := nextOccurrence(clockTime, now)
		entries = append(entries, scheduledAnnouncement{announcement: ann, next: next})
		slog.Info("npc-scheduler: scheduled announcement", "time", ann.Time, "next", next)
	}
	if len(entries) == 0 {
		slog.Info("npc-scheduler: no valid announcements configured, idling until config update")
	}
	return entries
}

// parseTime returns the elapsed time since midnight for HH:MM or HH:MM:SS.
func parseTime(s string) (time.Duration, bool) {
	parts := strings.Split(s, ":")
	if len(parts) < 2 || len(parts) > 3 {
		return 0, false
	}
	var values [3]time.Duration
	limits := [...]uint64{24, 60, 60}
	for i, part := range parts {
		value, err := strconv.ParseUint(strings.TrimPrefix(part, "+"), 10, 32)
		if err != nil || value >= limits[i] {
			return 0, false
		}
		values[i] = time.Duration(value)
	}
	return values[0]*time.Hour + values[1]*time.Minute + values[2]*time.Second, true
}

// Use a strictly future instant so a restart at the scheduled second does
// not fire it again. Like Rust, advancing an occurrence adds 24 elapsed hours.
func nextOccurrence(clockTime time.Duration, from time.Time) time.Time {
	year, month, day := from.Date()
	wall := time.Date(year, month, day, 0, 0, 0, 0, time.UTC).Add(clockTime)
	candidate := localDateTime(wall, from.Location())
	if !candidate.After(from) {
		candidate = candidate.Add(24 * time.Hour)
	}
	return candidate
}

// Go's time.Date may choose either offset at a fall-back transition. Try the
// nearby offsets to find the earlier match. A missing spring-forward time uses
// the wall clock fields as UTC, matching Chrono's fallback in the Rust port.
func localDateTime(wall time.Time, loc *time.Location) time.Time {
	year, month, day := wall.Date()
	hour, minute, second := wall.Clock()
	guess := time.Date(year, month, day, hour, minute, second, 0, loc)
	start, end := guess.ZoneBounds()
	nearby := []time.Time{guess}
	if !start.IsZero() {
		nearby = append(nearby, start.Add(-time.Nanosecond))
	}
	if !end.IsZero() {
		nearby = append(nearby, end)
	}
	var earliest time.Time
	for _, sample := range nearby {
		_, offset := sample.Zone()
		candidate := wall.Add(-time.Duration(offset) * time.Second).In(loc)
		_, actual := candidate.Zone()
		if actual == offset && (earliest.IsZero() || candidate.Before(earliest)) {
			earliest = candidate
		}
	}
	if earliest.IsZero() {
		return wall.In(loc)
	}
	return earliest
}
