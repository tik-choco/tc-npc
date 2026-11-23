package server

import (
	"encoding/json"
	"errors"
	"fmt"
	"time"

	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/config"
)

type moduleFlags struct {
	Talk        bool `json:"talk"`
	Memory      bool `json:"memory"`
	Speech      bool `json:"speech"`
	Vision      bool `json:"vision"`
	Action      bool `json:"action"`
	Scheduler   bool `json:"scheduler"`
	Translation bool `json:"translation"`
}

func flagsFor(cfg *config.Config) moduleFlags {
	return moduleFlags{
		Talk: cfg.Talk.Enabled, Memory: cfg.Memory.Enabled,
		Speech: cfg.TTS.Enabled || cfg.STT.Enabled, Vision: cfg.Vision.Enabled,
		Action: cfg.Action.Enabled, Scheduler: cfg.Scheduler.Enabled,
		Translation: !cfg.Translation.Mode().IsOff(),
	}
}

type helloFrame struct {
	Type      string            `json:"type"`
	Version   string            `json:"version"`
	Modules   moduleFlags       `json:"modules"`
	Character *characterRef     `json:"character"`
	Avatar    *character.Avatar `json:"avatar,omitempty"`
}

type chatFrame struct {
	Type string `json:"type"`
	Role string `json:"role"`
	Text string `json:"text"`
	TS   int64  `json:"ts"`
}

type responseFrame struct {
	Type      string  `json:"type"`
	RequestID string  `json:"requestId"`
	Status    string  `json:"status"`
	Text      *string `json:"text,omitempty"`
	Message   *string `json:"message,omitempty"`
}

type clientInput struct {
	Text    string  `json:"text" required:"true"`
	Speaker *string `json:"speaker"`
}

type clientCommand struct {
	Text string `json:"text" required:"true"`
}

type clientEvent struct {
	Kind        string   `json:"kind" required:"true"`
	UserName    *string  `json:"userName"`
	Text        *string  `json:"text"`
	Amount      *float64 `json:"amount"`
	Tier        *string  `json:"tier"`
	Message     *string  `json:"message"`
	RewardTitle *string  `json:"rewardTitle"`
}

type clientSignal string

func parseClientMessage(data []byte) (any, error) {
	var tag struct {
		Type string `json:"type" required:"true"`
	}
	if err := decodeFields(data, &tag, false); err != nil {
		return nil, err
	}
	switch tag.Type {
	case "input":
		var input clientInput
		err := decodeFields(data, &input, false)
		return input, err
	case "command":
		var command clientCommand
		err := decodeFields(data, &command, false)
		return command, err
	case "event":
		var event clientEvent
		err := decodeFields(data, &event, false)
		return event, err
	case "interrupt", "suspend", "resume", "voiceStart", "voiceStop":
		return clientSignal(tag.Type), nil
	default:
		return nil, fmt.Errorf("unknown variant %q, expected one of input, command, interrupt, suspend, resume, voiceStart, voiceStop, event", tag.Type)
	}
}

func nowMS() int64 { return time.Now().UnixMilli() }

type driveLevel struct {
	Key   string  `json:"key" required:"true"`
	Level float32 `json:"level" required:"true"`
	Base  float32 `json:"base" required:"true"`
}

func (d *driveLevel) UnmarshalJSON(data []byte) error {
	return decodeFields(data, d, true)
}

// affectSnapshot is both the WS payload and the history response entry
// (with a timestamp added by the forwarder).
type affectSnapshot struct {
	Familiarity     float32      `json:"familiarity"`
	Closing         bool         `json:"closing"`
	InviteCaution   bool         `json:"inviteCaution"`
	Partner         *string      `json:"partner,omitempty"`
	PartnerKnown    bool         `json:"partnerKnown"`
	PartnerSwitched bool         `json:"partnerSwitched"`
	PartnerAway     bool         `json:"partnerAway"`
	Drives          []driveLevel `json:"drives"`
}

func (a *affectSnapshot) UnmarshalJSON(data []byte) error {
	if err := decodeFields(data, a, true); err != nil {
		return err
	}
	if a.Drives == nil {
		a.Drives = []driveLevel{}
	}
	return nil
}

type affectEntry struct {
	TS int64 `json:"ts"`
	affectSnapshot
}

type affectFrame struct {
	Type string `json:"type"`
	affectEntry
}

func readPayload(raw json.RawMessage) (object, error) {
	if len(raw) == 0 {
		return nil, errors.New("empty bus payload")
	}
	value, err := decodeValue(raw)
	decoded, _ := value.(map[string]any)
	return decoded, err
}
