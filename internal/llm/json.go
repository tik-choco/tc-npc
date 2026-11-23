package llm

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
)

type fieldPresence uint8

const (
	required fieldPresence = iota
	defaulted
	optional
	requiredNullable
)

type jsonField struct {
	name     string
	target   any
	presence fieldPresence
}

// Serde matches field names exactly and distinguishes a missing field from an
// explicit null. Go's default struct decoder would also accept null for many
// non-optional fields and would silently replace duplicate fields.
func decodeObject(data []byte, fields ...jsonField) error {
	decoder := json.NewDecoder(bytes.NewReader(data))
	start, err := decoder.Token()
	if err != nil {
		return err
	}
	if start != json.Delim('{') {
		return fmt.Errorf("expected a JSON object")
	}

	values := make(map[string]json.RawMessage, len(fields))
	for decoder.More() {
		key, err := decoder.Token()
		if err != nil {
			return err
		}
		var value json.RawMessage
		if err := decoder.Decode(&value); err != nil {
			return err
		}
		for _, field := range fields {
			if field.name != key {
				continue
			}
			if _, exists := values[field.name]; exists {
				return fmt.Errorf("duplicate field %q", field.name)
			}
			values[field.name] = value
			break
		}
	}
	if _, err := decoder.Token(); err != nil {
		return err
	}
	if _, err := decoder.Token(); err != io.EOF {
		return fmt.Errorf("unexpected data after JSON object")
	}

	for _, field := range fields {
		value, exists := values[field.name]
		if !exists {
			if field.presence == required || field.presence == requiredNullable {
				return fmt.Errorf("missing field %q", field.name)
			}
			continue
		}
		if bytes.Equal(value, []byte("null")) && (field.presence == required || field.presence == defaulted) {
			return fmt.Errorf("field %q cannot be null", field.name)
		}
		if err := json.Unmarshal(value, field.target); err != nil {
			return fmt.Errorf("field %q: %w", field.name, err)
		}
	}
	return nil
}
