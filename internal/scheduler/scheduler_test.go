package scheduler

import (
	"context"
	"encoding/json"
	"testing"
	"testing/synctest"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/module"
)

func TestParseTime(t *testing.T) {
	for _, tc := range []struct {
		input string
		want  time.Duration
	}{
		{"09:30", 9*time.Hour + 30*time.Minute},
		{"09:30:15", 9*time.Hour + 30*time.Minute + 15*time.Second},
		{"+9:0:1", 9*time.Hour + time.Second},
		{"00:00", 0},
	} {
		t.Run(tc.input, func(t *testing.T) {
			got, ok := parseTime(tc.input)
			if !ok || got != tc.want {
				t.Fatalf("parseTime(%q) = %v, %v, want %v", tc.input, got, ok, tc.want)
			}
		})
	}
	for _, input := range []string{"25:00", "09:70", "not-a-time", "09", "09:30:15:99", "09:30:60", " 09:00", "09:-0"} {
		if _, ok := parseTime(input); ok {
			t.Errorf("parsed invalid time %q", input)
		}
	}
}

func local(day, hour, minute, second int) time.Time {
	return time.Date(2026, time.July, day, hour, minute, second, 0, time.Local)
}

func TestNextOccurrence(t *testing.T) {
	for _, tc := range []struct {
		name string
		from time.Time
		at   time.Duration
		want time.Time
	}{
		{"later today", local(24, 10, 0, 0), 15 * time.Hour, local(24, 15, 0, 0)},
		{"passed today", local(24, 10, 0, 0), 9 * time.Hour, local(25, 9, 0, 0)},
		{"exactly now", local(24, 10, 0, 0), 10 * time.Hour, local(25, 10, 0, 0)},
		{"seconds precision", local(24, 9, 0, 30), 9*time.Hour + 45*time.Second, local(24, 9, 0, 45)},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := nextOccurrence(tc.at, tc.from); !got.Equal(tc.want) {
				t.Fatalf("next occurrence = %v, want %v", got, tc.want)
			}
		})
	}
}

func TestNextOccurrenceAcrossDST(t *testing.T) {
	loc, err := time.LoadLocation("America/New_York")
	if err != nil {
		t.Skipf("timezone data unavailable: %v", err)
	}
	for _, tc := range []struct {
		name string
		from time.Time
		at   time.Duration
		want time.Time
	}{
		{
			"ambiguous time uses earlier instant",
			time.Date(2026, 11, 1, 0, 0, 0, 0, loc),
			time.Hour + 30*time.Minute,
			time.Date(2026, 11, 1, 5, 30, 0, 0, time.UTC),
		},
		{
			"nonexistent time uses UTC wall time and rolls forward",
			time.Date(2026, 3, 8, 0, 0, 0, 0, loc),
			2*time.Hour + 30*time.Minute,
			time.Date(2026, 3, 9, 2, 30, 0, 0, time.UTC),
		},
		{
			"rollover adds 24 elapsed hours",
			time.Date(2026, 3, 7, 12, 0, 0, 0, loc),
			9 * time.Hour,
			time.Date(2026, 3, 8, 10, 0, 0, 0, loc),
		},
	} {
		t.Run(tc.name, func(t *testing.T) {
			if got := nextOccurrence(tc.at, tc.from); !got.Equal(tc.want) || got.Location() != loc {
				t.Fatalf("next occurrence = %v, want %v in %v", got, tc.want, loc)
			}
		})
	}
}

func announcement(at string) config.AnnouncementConfig {
	ann := config.DefaultAnnouncementConfig()
	ann.Time = at
	ann.Text = "hello"
	ann.Volume = 1
	return ann
}

func TestComputeSchedule(t *testing.T) {
	for _, tc := range []struct {
		name    string
		enabled bool
		times   []string
		want    []string
	}{
		{"disabled", false, []string{"09:00"}, nil},
		{"skips invalid", true, []string{"09:00", "not-a-time"}, []string{"09:00"}},
		{"no announcements", true, nil, nil},
	} {
		t.Run(tc.name, func(t *testing.T) {
			cfg := config.DefaultSchedulerConfig()
			cfg.Enabled = tc.enabled
			for _, at := range tc.times {
				cfg.Announcements = append(cfg.Announcements, announcement(at))
			}
			entries := computeSchedule(cfg, local(24, 10, 0, 0))
			checkTimes(t, entries, tc.want...)
		})
	}
}

func TestComputeScheduleRecomputesFullyOnNewConfig(t *testing.T) {
	first := config.DefaultSchedulerConfig()
	first.Enabled = true
	first.Announcements = []config.AnnouncementConfig{announcement("09:00")}
	second := config.DefaultSchedulerConfig()
	second.Enabled = true
	second.Announcements = []config.AnnouncementConfig{announcement("10:00"), announcement("11:00")}

	now := local(24, 10, 0, 0)
	entries := computeSchedule(first, now)
	checkTimes(t, entries, "09:00")
	entries = computeSchedule(second, now)
	checkTimes(t, entries, "10:00", "11:00")
}

func checkTimes(t *testing.T, entries []scheduledAnnouncement, want ...string) {
	t.Helper()
	if len(entries) != len(want) {
		t.Fatalf("got %d entries, want %d", len(entries), len(want))
	}
	for i, entry := range entries {
		if got := entry.announcement.Time; got != want[i] {
			t.Errorf("entry %d time = %q, want %q", i, got, want[i])
		}
	}
}

func TestRunReloadsActivatedProfileAndDefaults(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		b := bus.New()
		observer := b.Subscribe()
		defer observer.Close()
		startup := config.DefaultConfig()
		at := time.Now().Add(2 * time.Second).Format("15:04:05")
		startup.Scheduler.Announcements = []config.AnnouncementConfig{announcement(at)}
		mc := module.Ctx{Bus: b, Config: &startup}
		m, err := New(mc)
		if err != nil || m.Name() != "npc-scheduler" {
			t.Fatalf("New() = %v, %v", m, err)
		}
		ctx, cancel := context.WithCancel(t.Context())
		defer cancel()
		done := make(chan error, 1)
		go func() { done <- m.Run(ctx, mc) }()
		synctest.Wait()

		updated := config.DefaultConfig()
		updated.ScheduleProfile.ActiveID = "event"
		updated.Scheduler.Enabled = true
		updated.Scheduler.Defaults.ChimeFile = "event.wav"
		for _, text := range []string{"first", "second"} {
			ann := announcement(at)
			ann.Text = text
			updated.Scheduler.Announcements = append(updated.Scheduler.Announcements, ann)
		}

		b.Publish(bus.TopicConfig, bus.MsgConfigUpdated, updated)
		receive(t, observer, bus.TopicConfig, bus.MsgConfigUpdated)
		b.Publish(bus.TopicConfig, bus.MsgConfigUpdated, json.RawMessage(`{"scheduler":null}`))
		receive(t, observer, bus.TopicConfig, bus.MsgConfigUpdated)
		synctest.Wait()
		noMessage(t, observer)
		time.Sleep(2 * time.Second)
		synctest.Wait()
		for _, text := range []string{"first", "second"} {
			msg := receive(t, observer, bus.TopicInterrupt, bus.MsgTTS)
			if msg.Str("content") != text || msg.Str("chime_file") != "event.wav" {
				t.Fatalf("payload = %s, want %q with event.wav", msg.Payload, text)
			}
		}

		// Recomputing at the exact scheduled second must not fire it again.
		b.Publish(bus.TopicConfig, bus.MsgConfigUpdated, updated)
		receive(t, observer, bus.TopicConfig, bus.MsgConfigUpdated)
		synctest.Wait()
		time.Sleep(10 * time.Second)
		synctest.Wait()
		noMessage(t, observer)

		cancel()
		if err := <-done; err != nil {
			t.Fatal(err)
		}
	})
}
