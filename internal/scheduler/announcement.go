package scheduler

import (
	"bytes"
	"log/slog"
	"math"
	"strings"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
)

// FireOutcome reports what an announcement published.
type FireOutcome struct {
	Spoke   bool // The announcement's own text produced a TTS interrupt.
	Actions int  // Number of scheduled actions published.
}

// Fired reports whether the announcement published anything.
func (o FireOutcome) Fired() bool { return o.Spoke || o.Actions > 0 }

// FireAnnouncement publishes the announcement's own TTS line, if present,
// followed by any actions. The clock and the HTTP test endpoint share this path.
func FireAnnouncement(b *bus.Bus, ann *config.AnnouncementConfig, defaults *config.SchedulerDefaults) FireOutcome {
	var outcome FireOutcome
	if ann.Text == "" {
		slog.Debug("npc-scheduler: announcement has no text, skipping tts", "time", ann.Time)
	} else {
		slog.Info("npc-scheduler: firing announcement", "time", ann.Time, "text", ann.Text)
		b.Publish(bus.TopicInterrupt, bus.MsgTTS, map[string]any{
			"content":       ann.Text,
			"chime_file":    ann.EffectiveChimeFile(defaults),
			"volume":        jsonVolume(ann.EffectiveVolume(defaults)),
			"bgm_file":      ann.BGMFile,
			"bgm_volume":    jsonVolume(ann.EffectiveBGMVolume(defaults)),
			"bgm_play_full": ann.BGMPlayFull,
			"bgm_end_time":  ann.BGMEndTime,
		})
		outcome.Spoke = true
	}

	for i := range ann.Actions {
		if ExecuteAction(b, &ann.Actions[i]) {
			outcome.Actions++
		}
	}
	if !outcome.Fired() {
		slog.Debug("npc-scheduler: no-op announcement (no text, no actions)", "time", ann.Time)
	}
	return outcome
}

// Rust's conversion into a JSON value widens f32 and uses null for nonfinite
// values. Widen before encoding to preserve the precision of the bus payload.
func jsonVolume(volume float32) any {
	value := float64(volume)
	if math.IsNaN(value) || math.IsInf(value, 0) {
		return nil
	}
	return value
}

// ExecuteAction publishes the action's envelope and reports whether it
// published anything. Blank fields in unfinished UI rows are skipped.
func ExecuteAction(b *bus.Bus, action *config.ScheduledAction) bool {
	switch action.Kind {
	case config.ScheduledActionSpeak:
		if action.Content == "" && action.ChimeFile == "" {
			return false
		}
		b.Publish(bus.TopicInterrupt, bus.MsgTTS, map[string]string{"content": action.Content, "chime_file": action.ChimeFile})
	case config.ScheduledActionAction:
		if strings.TrimSpace(action.Content) == "" {
			return false
		}
		b.Publish(bus.TopicAction, bus.MsgAction, map[string]string{"content": action.Content})
	case config.ScheduledActionCommand:
		if strings.TrimSpace(action.Text) == "" {
			return false
		}
		// "command" is npc-action's local message type.
		b.Publish(bus.TopicAction, "command", map[string]string{"text": action.Text})
	case config.ScheduledActionChat:
		if strings.TrimSpace(action.Content) == "" {
			return false
		}
		b.Publish(bus.TopicSense, bus.MsgSpeech, map[string]string{"content": action.Content})
	case config.ScheduledActionSuspend:
		b.Publish(bus.TopicInterrupt, bus.MsgSuspend, struct{}{})
	case config.ScheduledActionResume:
		b.Publish(bus.TopicInterrupt, bus.MsgResume, struct{}{})
	case config.ScheduledActionRaw:
		if strings.TrimSpace(action.Topic) == "" || strings.TrimSpace(action.MessageType) == "" {
			return false
		}
		b.Publish(action.Topic, action.MessageType, bytes.Clone(action.Payload))
	default:
		return false
	}
	slog.Info("npc-scheduler: firing scheduled action", "action", action)
	return true
}
