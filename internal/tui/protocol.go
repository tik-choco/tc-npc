package tui

import (
	"encoding/json"
	"fmt"
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

type characterRef struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

func (m *moduleFlags) UnmarshalJSON(data []byte) error {
	return decodeObject(data,
		frameField{"talk", &m.Talk, false}, frameField{"memory", &m.Memory, false},
		frameField{"speech", &m.Speech, false}, frameField{"vision", &m.Vision, false},
		frameField{"action", &m.Action, false}, frameField{"scheduler", &m.Scheduler, false},
		frameField{"translation", &m.Translation, false},
	)
}

func (c *characterRef) UnmarshalJSON(data []byte) error {
	return decodeObject(data, frameField{"id", &c.ID, false}, frameField{"name", &c.Name, false})
}

// serverMsg contains the fields consumed by the TUI. Unmodeled frames retain
// their type for logging, so adding a server frame need not change the client.
type serverMsg struct {
	kind    string
	unknown bool

	version   string
	modules   moduleFlags
	character *characterRef
	role      string
	text      *string
	ts        int64
	reason    string
	category  string
	requestID string
	status    string
	message   *string
}

type frameField struct {
	name     string
	dst      any
	optional bool
}

func decodeObject(data []byte, fields ...frameField) error {
	var object map[string]json.RawMessage
	if err := json.Unmarshal(data, &object); err != nil {
		return err
	}
	return decodeFields(object, fields)
}

// Match serde's exact field names and distinguish missing or null required
// fields from valid zero values.
func decodeFields(object map[string]json.RawMessage, fields []frameField) error {
	for _, field := range fields {
		raw := object[field.name]
		if raw == nil || string(raw) == "null" {
			if field.optional {
				continue
			}
			return fmt.Errorf("missing or null %q field", field.name)
		}
		if err := json.Unmarshal(raw, field.dst); err != nil {
			return fmt.Errorf("decoding %q field: %w", field.name, err)
		}
	}
	return nil
}

func (m *serverMsg) UnmarshalJSON(data []byte) error {
	var object map[string]json.RawMessage
	if err := json.Unmarshal(data, &object); err != nil {
		return err
	}
	var kind *string
	if raw := object["type"]; raw != nil {
		if err := json.Unmarshal(raw, &kind); err != nil {
			return fmt.Errorf("reading frame type: %w", err)
		}
	}
	if kind == nil {
		return fmt.Errorf("expected a frame with a string type")
	}

	msg := serverMsg{kind: *kind}
	unknown := serverMsg{kind: *kind, unknown: true}
	var fields []frameField
	switch msg.kind {
	case "hello":
		fields = []frameField{{"version", &msg.version, false}, {"modules", &msg.modules, false}, {"character", &msg.character, true}}
	case "status":
		fields = []frameField{{"modules", &msg.modules, false}}
	case "chat":
		fields = []frameField{{"role", &msg.role, false}, {"text", &msg.text, false}, {"ts", &msg.ts, false}}
	case "silent":
		fields = []frameField{{"reason", &msg.reason, false}, {"ts", &msg.ts, false}}
	case "sense":
		fields = []frameField{{"kind", &msg.category, false}, {"text", &msg.text, false}, {"ts", &msg.ts, false}}
	case "memory":
		fields = []frameField{{"kind", &msg.category, false}, {"text", &msg.text, false}}
	case "actionLog":
		fields = []frameField{{"text", &msg.text, false}}
	case "error":
		fields = []frameField{{"message", &msg.message, false}}
	case "inputAccepted":
		fields = []frameField{{"requestId", &msg.requestID, false}}
	case "response":
		fields = []frameField{{"requestId", &msg.requestID, false}, {"status", &msg.status, false}, {"text", &msg.text, true}, {"message", &msg.message, true}}
	default:
		*m = unknown
		return nil
	}

	// The Rust parser also falls back to the type name when a known frame
	// has an incompatible payload. Only an invalid envelope is a hard error.
	if err := decodeFields(object, fields); err != nil {
		msg = unknown
	}
	*m = msg
	return nil
}

// clientMsg is the single outgoing variant, a chat input. The TUI has no
// login identity and leaves Speaker nil.
type clientMsg struct {
	Text    string
	Speaker *string
}

func (m clientMsg) MarshalJSON() ([]byte, error) {
	return json.Marshal(struct {
		Type    string  `json:"type"`
		Text    string  `json:"text"`
		Speaker *string `json:"speaker,omitempty"`
	}{Type: "input", Text: m.Text, Speaker: m.Speaker})
}
