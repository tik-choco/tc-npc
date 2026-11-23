package memory

import (
	"encoding/json"
	"errors"
	"strings"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/chatlog"
)

// A missing field defaults to the empty string, while explicit null is
// invalid for a non-optional Rust string.
type defaultString string

func (s *defaultString) UnmarshalJSON(data []byte) error {
	var value *string
	if err := json.Unmarshal(data, &value); err != nil {
		return err
	}
	if value == nil {
		return errors.New("expected a string")
	}
	*s = defaultString(*value)
	return nil
}

type chatLogPayload struct {
	Input  defaultString `json:"input"`
	Output defaultString `json:"output"`
}

func parseChatLog(msg bus.Message) string {
	var payload chatLogPayload
	if err := msg.Decode(&payload); err != nil || (payload.Input == "" && payload.Output == "") {
		return ""
	}
	return "User: " + string(payload.Input) + "\nAI: " + string(payload.Output)
}

// The summary parser reads just the turn. Bad optional transcript metadata
// can prevent the raw entry from being built without invalidating the turn.
func buildChatLogEntry(msg bus.Message) *chatlog.ChatLogEntry {
	var payload struct {
		chatLogPayload
		Time    *string `json:"time"`
		Speaker *string `json:"speaker"`
	}
	if err := msg.Decode(&payload); err != nil || (payload.Input == "" && payload.Output == "") {
		return nil
	}
	var timestamp string
	if payload.Time == nil || *payload.Time == "" {
		timestamp = formatTimestamp(time.Now().UTC())
	} else {
		timestamp = *payload.Time
	}
	if payload.Speaker != nil && *payload.Speaker == "" {
		payload.Speaker = nil
	}
	return &chatlog.ChatLogEntry{
		Time: timestamp, Speaker: payload.Speaker, Input: string(payload.Input), Output: string(payload.Output),
	}
}

func formatTimestamp(t time.Time) string {
	// Match Chrono's RFC3339 precision and explicit UTC offset.
	const dateTime = "2006-01-02T15:04:05"
	const offset = "-07:00"
	nanoseconds := t.Nanosecond()
	switch {
	case nanoseconds == 0:
		return t.Format(dateTime + offset)
	case nanoseconds%1_000_000 == 0:
		return t.Format(dateTime + ".000" + offset)
	case nanoseconds%1_000 == 0:
		return t.Format(dateTime + ".000000" + offset)
	default:
		return t.Format(dateTime + ".000000000" + offset)
	}
}

func parseSenseContent(msg bus.Message) string {
	var content string
	if err := msg.Decode(&content); err == nil {
		return content
	}
	return msg.Str("content")
}

func parseSpeaker(msg bus.Message) string {
	return strings.TrimSpace(msg.Str("speaker"))
}

type personSeenPayload struct {
	Name       defaultString `json:"name"`
	Appearance defaultString `json:"appearance"`
	Note       defaultString `json:"note"`
}

func parsePersonSeen(msg bus.Message) *personSeenPayload {
	var seen *personSeenPayload
	if err := msg.Decode(&seen); err != nil {
		return nil
	}
	return seen
}

type extractedPersonFacts struct {
	Name  string
	Facts []string
}

func (f *extractedPersonFacts) UnmarshalJSON(data []byte) error {
	type fields struct {
		Name  defaultString `json:"name"`
		Facts []*string     `json:"facts"`
	}
	value := &fields{Facts: []*string{}}
	if err := json.Unmarshal(data, &value); err != nil {
		return err
	}
	if value == nil || value.Facts == nil {
		return errors.New("expected a person facts object with an array of facts")
	}
	facts := make([]string, len(value.Facts))
	for i, text := range value.Facts {
		if text == nil {
			return errors.New("expected fact text")
		}
		facts[i] = *text
	}
	*f = extractedPersonFacts{Name: string(value.Name), Facts: facts}
	return nil
}

// Models can surround the array with prose or a code fence. Decode the
// first array as JSON so brackets and escapes in fact strings are respected.
func parsePersonFactsResponse(response string) []extractedPersonFacts {
	start := strings.IndexByte(response, '[')
	if start < 0 {
		return nil
	}
	var extracted []extractedPersonFacts
	if err := json.NewDecoder(strings.NewReader(response[start:])).Decode(&extracted); err != nil {
		return nil
	}
	return extracted
}
