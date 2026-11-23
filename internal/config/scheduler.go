package config

import (
	"encoding/json"
	"fmt"
)

// ScheduledActionKind is the discriminator in an announcement action.
type ScheduledActionKind string

const (
	ScheduledActionSpeak   ScheduledActionKind = "speak"
	ScheduledActionAction  ScheduledActionKind = "action"
	ScheduledActionCommand ScheduledActionKind = "command"
	ScheduledActionChat    ScheduledActionKind = "chat"
	ScheduledActionSuspend ScheduledActionKind = "suspend"
	ScheduledActionResume  ScheduledActionKind = "resume"
	ScheduledActionRaw     ScheduledActionKind = "raw"
)

// ScheduledAction publishes a bus message when its announcement fires.
// Speak uses Content and ChimeFile, Action and Chat use Content, Command
// uses Text, and Raw uses Topic, MessageType and Payload. Suspend and
// Resume carry no additional fields.
type ScheduledAction struct {
	Kind        ScheduledActionKind
	Content     string
	ChimeFile   string
	Text        string
	Topic       string
	MessageType string
	Payload     json.RawMessage
}

type scheduledSpeak struct {
	Kind      ScheduledActionKind `json:"kind"`
	Content   string              `json:"content"`
	ChimeFile string              `json:"chime_file"`
}

type scheduledContent struct {
	Kind    ScheduledActionKind `json:"kind"`
	Content string              `json:"content"`
}

type scheduledCommand struct {
	Kind ScheduledActionKind `json:"kind"`
	Text string              `json:"text"`
}

type scheduledUnit struct {
	Kind ScheduledActionKind `json:"kind"`
}

type scheduledRaw struct {
	Kind        ScheduledActionKind `json:"kind"`
	Topic       string              `json:"topic" required:"true"`
	MessageType string              `json:"type" required:"true"`
	Payload     json.RawMessage     `json:"payload"`
}

func (a ScheduledAction) MarshalJSON() ([]byte, error) {
	switch a.Kind {
	case ScheduledActionSpeak:
		return json.Marshal(scheduledSpeak{Kind: a.Kind, Content: a.Content, ChimeFile: a.ChimeFile})
	case ScheduledActionAction, ScheduledActionChat:
		return json.Marshal(scheduledContent{Kind: a.Kind, Content: a.Content})
	case ScheduledActionCommand:
		return json.Marshal(scheduledCommand{Kind: a.Kind, Text: a.Text})
	case ScheduledActionSuspend, ScheduledActionResume:
		return json.Marshal(scheduledUnit{Kind: a.Kind})
	case ScheduledActionRaw:
		return json.Marshal(scheduledRaw{Kind: a.Kind, Topic: a.Topic, MessageType: a.MessageType, Payload: a.Payload})
	default:
		return nil, fmt.Errorf("unknown scheduled action kind %q", a.Kind)
	}
}

func (a *ScheduledAction) UnmarshalJSON(data []byte) error {
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		return fmt.Errorf("expected a tagged scheduled action: %w", err)
	}
	tag, ok := fields["kind"]
	if !ok {
		return fmt.Errorf("missing scheduled action kind")
	}
	var action ScheduledAction
	if err := json.Unmarshal(tag, &action.Kind); err != nil {
		return fmt.Errorf("invalid scheduled action kind: %w", err)
	}
	switch action.Kind {
	case ScheduledActionSpeak:
		var wire scheduledSpeak
		if err := decodeStruct(data, &wire, scheduledSpeak{}); err != nil {
			return err
		}
		action.Content, action.ChimeFile = wire.Content, wire.ChimeFile
	case ScheduledActionAction, ScheduledActionChat:
		var wire scheduledContent
		if err := decodeStruct(data, &wire, scheduledContent{}); err != nil {
			return err
		}
		action.Content = wire.Content
	case ScheduledActionCommand:
		var wire scheduledCommand
		if err := decodeStruct(data, &wire, scheduledCommand{}); err != nil {
			return err
		}
		action.Text = wire.Text
	case ScheduledActionSuspend, ScheduledActionResume:
		var wire scheduledUnit
		if err := decodeStruct(data, &wire, scheduledUnit{}); err != nil {
			return err
		}
	case ScheduledActionRaw:
		var wire scheduledRaw
		if err := decodeStruct(data, &wire, scheduledRaw{Payload: json.RawMessage("null")}); err != nil {
			return err
		}
		// Keep raw payloads compact so a decode/encode/decode preserves their
		// representation even if the source document was pretty-printed.
		compact, err := json.Marshal(wire.Payload)
		if err != nil {
			return fmt.Errorf("invalid scheduled action payload: %w", err)
		}
		action.Topic, action.MessageType, action.Payload = wire.Topic, wire.MessageType, compact
	default:
		return fmt.Errorf("unknown scheduled action kind %q", action.Kind)
	}
	*a = action
	return nil
}

// EffectiveChimeFile falls back to the scheduler default when ChimeFile is empty.
func (a *AnnouncementConfig) EffectiveChimeFile(defaults *SchedulerDefaults) string {
	if a.ChimeFile == "" {
		return defaults.ChimeFile
	}
	return a.ChimeFile
}

// EffectiveVolume falls back to the scheduler default for a nonpositive Volume.
func (a *AnnouncementConfig) EffectiveVolume(defaults *SchedulerDefaults) float32 {
	if a.Volume <= 0 {
		return defaults.Volume
	}
	return a.Volume
}

// EffectiveBGMVolume falls back to the scheduler default for a nonpositive BGMVolume.
func (a *AnnouncementConfig) EffectiveBGMVolume(defaults *SchedulerDefaults) float32 {
	if a.BGMVolume <= 0 {
		return defaults.BGMVolume
	}
	return a.BGMVolume
}
