package mist

import (
	"bytes"
	"encoding/json"
	"fmt"
	"math"
	"slices"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/google/uuid"
)

const avatarLinkVersion = 1

var avatarLinkCaps = []string{"hello", "bye", "input", "state", "say", "silent", "speaking", "mouth", "stop"}

type hostEnvelope struct {
	id, from, kind, text string
	speaker, to          *string
}

func decodeHost(data []byte) (hostEnvelope, bool) {
	var host hostEnvelope
	var fields map[string]json.RawMessage
	if !utf8.Valid(data) || json.Unmarshal(data, &fields) != nil {
		return host, false
	}
	var (
		version uint8
		role    string
		ts      *uint64
		id      *string
		from    *string
	)
	if err := decodeFields(fields, map[string]any{
		"v": &version, "role": &role, "ts": &ts, "id": &id, "from": &from, "t": &host.kind,
	}); err != nil || version != avatarLinkVersion || role != "host" || ts == nil || id == nil || from == nil {
		return host, false
	}
	host.id, host.from = *id, *from
	switch host.kind {
	case "hello":
		var name *string
		var caps []*string
		err := decodeFields(fields, map[string]any{"name": &name, "caps": &caps})
		return host, err == nil && name != nil && caps != nil && !slices.Contains(caps, nil)
	case "bye":
		return host, true
	case "input":
		var text *string
		if err := decodeFields(fields, map[string]any{
			"text": &text, "speaker": &host.speaker, "to": &host.to,
		}); err != nil || text == nil {
			return host, false
		}
		host.text = *text
		return host, true
	case "state":
		var speaking *bool
		err := decodeFields(fields, map[string]any{"speaking": &speaking})
		return host, err == nil && speaking != nil
	default:
		return host, false
	}
}

// Match JSON keys exactly, as serde does, and leave absent fields at their defaults.
func decodeFields(fields map[string]json.RawMessage, destinations map[string]any) error {
	for name, dst := range destinations {
		if raw, ok := fields[name]; ok {
			if _, required := dst.(*string); required && bytes.Equal(raw, []byte("null")) {
				return fmt.Errorf("%s: expected a string", name)
			}
			if err := json.Unmarshal(raw, dst); err != nil {
				return fmt.Errorf("%s: %w", name, err)
			}
		}
	}
	return nil
}

type audio string

const (
	audioBrain audio = "brain"
	audioHost  audio = "host"
)

// Each variant supplies its own fields. Pointers preserve required empty
// strings and false booleans as well as optional empty reply IDs and reasons.
type brainMessage struct {
	Kind      string      `json:"t"`
	Name      *string     `json:"name,omitempty"`
	Caps      []string    `json:"caps,omitempty"`
	LineID    *uuid.UUID  `json:"lineId,omitempty"`
	Text      *string     `json:"text,omitempty"`
	Audio     audio       `json:"audio,omitempty"`
	InReplyTo *string     `json:"inReplyTo,omitempty"`
	Reason    *string     `json:"reason,omitempty"`
	Active    *bool       `json:"active,omitempty"`
	Level     json.Number `json:"level,omitempty"`
}

func encodeBrain(from string, id uuid.UUID, ts uint64, message brainMessage) ([]byte, error) {
	return encodeJSON(struct {
		Version uint8     `json:"v"`
		ID      uuid.UUID `json:"id"`
		From    string    `json:"from"`
		Role    string    `json:"role"`
		TS      uint64    `json:"ts"`
		brainMessage
	}{avatarLinkVersion, id, from, "brain", ts, message}, "")
}

func mouthLevel(level float64) json.Number {
	// Serde emits floats with a decimal point for integral levels and switches
	// to scientific notation below a decimal exponent of -5.
	if mag := math.Abs(level); mag > 0 && mag < 1e-5 {
		return json.Number(strings.Replace(strconv.FormatFloat(level, 'e', -1, 64), "e-0", "e-", 1))
	}
	text := strconv.FormatFloat(level, 'f', -1, 64)
	if !strings.ContainsRune(text, '.') {
		text += ".0"
	}
	return json.Number(text)
}

func encodeJSON(value any, indent string) ([]byte, error) {
	var buffer bytes.Buffer
	enc := json.NewEncoder(&buffer)
	enc.SetEscapeHTML(false)
	enc.SetIndent("", indent)
	if err := enc.Encode(value); err != nil {
		return nil, err
	}
	data := buffer.Bytes()[:buffer.Len()-1] // The encoder adds a trailing newline.
	// encoding/json escapes these Unicode separators even with HTML escaping
	// disabled. Serde writes them directly. Skip escaped backslashes so a
	// literal string containing a backslash followed by u2028 stays literal.
	out := data[:0]
	for i := 0; i < len(data); i++ {
		if data[i] == '\\' {
			escape := data[i:]
			switch {
			case bytes.HasPrefix(escape, []byte(`\u2028`)):
				out = append(out, "\u2028"...)
				i += 5
			case bytes.HasPrefix(escape, []byte(`\u2029`)):
				out = append(out, "\u2029"...)
				i += 5
			default:
				out = append(out, data[i:i+2]...)
				i++
			}
			continue
		}
		out = append(out, data[i])
	}
	return out, nil
}
