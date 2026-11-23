package mistclient

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"reflect"
	"strings"
	"unicode/utf8"
)

func (r Registration) MarshalJSON() ([]byte, error) {
	type wire Registration
	r.Providers = jsonArray(r.Providers)
	r.Rooms = jsonArray(r.Rooms)
	return encodeJSON(wire(r))
}

func (r *Registration) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, r, Registration{})
}

func (p *ExternalProvider) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, p, ExternalProvider{})
}

func (r ExternalRoom) MarshalJSON() ([]byte, error) {
	type wire ExternalRoom
	r.Shared = jsonArray(r.Shared)
	return encodeJSON(wire(r))
}

func (r *ExternalRoom) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, r, ExternalRoom{})
}

func (r RoomStatus) MarshalJSON() ([]byte, error) {
	type wire RoomStatus
	r.Models = jsonArray(r.Models)
	return encodeJSON(wire(r))
}

func (r *RoomStatus) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, r, RoomStatus{Models: []string{}})
}

func (r AppliedRegistration) MarshalJSON() ([]byte, error) {
	type wire AppliedRegistration
	r.Rooms = jsonArray(r.Rooms)
	return encodeJSON(wire(r))
}

func (r *AppliedRegistration) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, r, AppliedRegistration{
		Rooms:  []ExternalRoom{},
		Status: ExternalStatus{Rooms: []RoomStatus{}},
	})
}

func (s ExternalStatus) MarshalJSON() ([]byte, error) {
	type wire ExternalStatus
	s.Rooms = jsonArray(s.Rooms)
	return encodeJSON(wire(s))
}

func (s *ExternalStatus) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, s, ExternalStatus{Rooms: []RoomStatus{}})
}

func (g ExternalGet) MarshalJSON() ([]byte, error) {
	type wire ExternalGet
	g.Registrations = jsonArray(g.Registrations)
	return encodeJSON(wire(g))
}

func (g *ExternalGet) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, g, ExternalGet{})
}

func (r *ApplyResult) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, r, ApplyResult{Warnings: []string{}})
}

func (r *removeResult) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, r, removeResult{})
}

var lineSeparators = strings.NewReplacer(`\\`, `\\`, `\u2028`, "\u2028", `\u2029`, "\u2029")

func encodeJSON(value any) ([]byte, error) {
	var buffer bytes.Buffer
	encoder := json.NewEncoder(&buffer)
	encoder.SetEscapeHTML(false)
	if err := encoder.Encode(value); err != nil {
		return nil, err
	}
	data := bytes.TrimSuffix(buffer.Bytes(), []byte("\n"))
	// encoding/json also escapes these two characters with HTML escaping off.
	// Keep escaped backslashes intact when restoring the original UTF-8.
	if bytes.Contains(data, []byte(`\u202`)) {
		return []byte(lineSeparators.Replace(string(data))), nil
	}
	return data, nil
}

// Rust Vec fields always serialize as arrays, even when empty.
func jsonArray[T any](values []T) []T {
	if values == nil {
		return []T{}
	}
	return values
}

// As in the config schema, serde structs accept objects or positional arrays,
// match field names exactly, and default fields only when they are absent.
func decodeStruct[T any](data []byte, target *T, defaults T) error {
	if !utf8.Valid(data) {
		return fmt.Errorf("invalid UTF-8")
	}
	decoder := json.NewDecoder(bytes.NewReader(data))
	start, err := decoder.Token()
	if err != nil {
		return err
	}
	if start != json.Delim('{') && start != json.Delim('[') {
		return fmt.Errorf("expected a JSON object or array")
	}

	value := reflect.ValueOf(&defaults).Elem()
	typ := value.Type()
	names := make([]string, typ.NumField())
	fields := make(map[string]int, len(names))
	for i := range names {
		names[i], _, _ = strings.Cut(typ.Field(i).Tag.Get("json"), ",")
		fields[names[i]] = i
	}
	seen := make([]bool, len(names))
	for position := 0; decoder.More(); position++ {
		index := position
		known := true
		if start == json.Delim('{') {
			key, err := decoder.Token()
			if err != nil {
				return err
			}
			index, known = fields[key.(string)]
		} else if index >= len(names) {
			return fmt.Errorf("too many fields for %s", typ.Name())
		}
		var raw json.RawMessage
		if err := decoder.Decode(&raw); err != nil {
			return err
		}
		if !known {
			continue
		}
		if seen[index] {
			return fmt.Errorf("duplicate field %q", names[index])
		}
		if err := decodeField(raw, value.Field(index).Addr().Interface()); err != nil {
			return fmt.Errorf("field %q: %w", names[index], err)
		}
		seen[index] = true
	}
	if _, err := decoder.Token(); err != nil {
		return err
	}
	if _, err := decoder.Token(); err != io.EOF {
		return fmt.Errorf("unexpected data after %s", typ.Name())
	}
	for i, name := range names {
		if !seen[i] && typ.Field(i).Tag.Get("required") == "true" {
			return fmt.Errorf("missing field %q", name)
		}
	}
	*target = defaults
	return nil
}

func decodeField(raw json.RawMessage, target any) error {
	if bytes.Equal(raw, []byte("null")) {
		if _, optional := target.(**string); !optional {
			return fmt.Errorf("cannot be null")
		}
	}
	if stringsField, ok := target.(*[]string); ok {
		// The standard decoder accepts null as a string's zero value.
		var entries []*string
		if err := json.Unmarshal(raw, &entries); err != nil {
			return err
		}
		values := make([]string, len(entries))
		for i, entry := range entries {
			if entry == nil {
				return fmt.Errorf("entry %d cannot be null", i)
			}
			values[i] = *entry
		}
		*stringsField = values
		return nil
	}
	return json.Unmarshal(raw, target)
}
