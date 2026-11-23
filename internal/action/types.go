package action

import (
	"bytes"
	"encoding/json"
	"fmt"
)

type actionItem struct {
	Name string   `json:"name"`
	Args []string `json:"args"`
}

func (a actionItem) MarshalJSON() ([]byte, error) {
	if a.Args == nil {
		a.Args = []string{}
	}
	type wire actionItem
	return json.Marshal(wire(a))
}

func (a *actionItem) UnmarshalJSON(data []byte) error {
	var name string
	var args []*string
	if err := decodePlanFields(data, planField{"name", &name, true}, planField{"args", &args, false}); err != nil {
		return err
	}
	item := actionItem{Name: name, Args: make([]string, 0, len(args))}
	for _, arg := range args {
		if arg == nil {
			return fmt.Errorf("action argument cannot be null")
		}
		item.Args = append(item.Args, *arg)
	}
	*a = item
	return nil
}

type actionResponse struct {
	Message string       `json:"message"`
	Actions []actionItem `json:"actions"`
}

func (a *actionResponse) UnmarshalJSON(data []byte) error {
	response := actionResponse{Actions: []actionItem{}}
	if err := decodePlanFields(data, planField{"message", &response.Message, false}, planField{"actions", &response.Actions, false}); err != nil {
		return err
	}
	*a = response
	return nil
}

type planField struct {
	name     string
	target   any
	required bool
}

// Serde accepts named fields or their positional representation. Known fields
// are case-sensitive, may appear only once, and cannot be null.
func decodePlanFields(data []byte, fields ...planField) error {
	decoder := json.NewDecoder(bytes.NewReader(data))
	start, err := decoder.Token()
	if err != nil {
		return err
	}
	seen := make([]bool, len(fields))
	switch start {
	case json.Delim('{'):
		for decoder.More() {
			key, err := decoder.Token()
			if err != nil {
				return err
			}
			var raw json.RawMessage
			if err := decoder.Decode(&raw); err != nil {
				return err
			}
			for i, field := range fields {
				if key != field.name {
					continue
				}
				if seen[i] {
					return fmt.Errorf("duplicate field %q", field.name)
				}
				if err := field.decode(raw); err != nil {
					return err
				}
				seen[i] = true
				break
			}
		}
	case json.Delim('['):
		for i := 0; decoder.More(); i++ {
			if i >= len(fields) {
				return fmt.Errorf("too many positional action fields")
			}
			var raw json.RawMessage
			if err := decoder.Decode(&raw); err != nil {
				return err
			}
			if err := fields[i].decode(raw); err != nil {
				return err
			}
			seen[i] = true
		}
	default:
		return fmt.Errorf("expected an action object or array")
	}
	if _, err := decoder.Token(); err != nil {
		return err
	}
	for i, field := range fields {
		if field.required && !seen[i] {
			return fmt.Errorf("missing field %q", field.name)
		}
	}
	return nil
}

func (f planField) decode(raw json.RawMessage) error {
	if bytes.Equal(raw, []byte("null")) {
		return fmt.Errorf("field %q cannot be null", f.name)
	}
	if err := json.Unmarshal(raw, f.target); err != nil {
		return fmt.Errorf("field %q: %w", f.name, err)
	}
	return nil
}
