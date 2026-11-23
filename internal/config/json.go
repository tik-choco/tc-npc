package config

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"reflect"
	"strconv"
	"strings"
	"unicode/utf8"
)

// decodeStruct applies serde's struct rules. JSON tags supply the wire names,
// input tags mark deserialize-only fields, and required tags reject omissions.
// Decode each field individually for exact case, duplicate and null handling.
func decodeStruct[T any](data []byte, target *T, defaults T) error {
	if !utf8.Valid(data) {
		return fmt.Errorf("config JSON contains invalid UTF-8")
	}
	value := reflect.ValueOf(&defaults).Elem()
	typ := value.Type()
	names := make([]string, typ.NumField())
	fields := make(map[string]int, len(names))
	for i := range names {
		tag := typ.Field(i).Tag
		name := jsonName(tag)
		if name == "-" {
			name = tag.Get("input")
		}
		names[i] = name
		fields[name] = i
	}

	decoder := json.NewDecoder(bytes.NewReader(data))
	start, err := decoder.Token()
	if err != nil {
		return err
	}
	seen := make([]bool, len(names))
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
			i, ok := fields[key.(string)]
			if !ok {
				continue
			}
			if seen[i] {
				return fmt.Errorf("duplicate config field %q", names[i])
			}
			if err := decodeField(names[i], raw, value.Field(i)); err != nil {
				return err
			}
			seen[i] = true
		}
	case json.Delim('['):
		// Derived serde structs also accept positional sequences.
		for i := 0; decoder.More(); i++ {
			if i >= len(names) {
				return fmt.Errorf("too many fields for %s", typ.Name())
			}
			var raw json.RawMessage
			if err := decoder.Decode(&raw); err != nil {
				return err
			}
			if err := decodeField(names[i], raw, value.Field(i)); err != nil {
				return err
			}
			seen[i] = true
		}
	default:
		return fmt.Errorf("expected a JSON object or array for %s", typ.Name())
	}
	if _, err := decoder.Token(); err != nil {
		return err
	}
	if _, err := decoder.Token(); err != io.EOF {
		return fmt.Errorf("unexpected data after %s", typ.Name())
	}
	for i, name := range names {
		if !seen[i] && typ.Field(i).Tag.Get("required") == "true" {
			return fmt.Errorf("missing config field %q", name)
		}
	}
	*target = defaults
	return nil
}

func jsonName(tag reflect.StructTag) string {
	name, _, _ := strings.Cut(tag.Get("json"), ",")
	return name
}

// serde_json::to_value widens f32 fields to f64. Replicate that conversion
// for the detached redacted JSON value and keep integers at full precision.
func widenJSONFloats(value reflect.Value, node any) any {
	switch value.Kind() {
	case reflect.Pointer:
		if !value.IsNil() {
			return widenJSONFloats(value.Elem(), node)
		}
	case reflect.Float32:
		return json.Number(strconv.FormatFloat(value.Float(), 'g', -1, 64))
	case reflect.Struct:
		if object, ok := node.(map[string]any); ok {
			typ := value.Type()
			for i := range value.NumField() {
				name := jsonName(typ.Field(i).Tag)
				if child, exists := object[name]; exists && name != "" && name != "-" {
					object[name] = widenJSONFloats(value.Field(i), child)
				}
			}
		}
	case reflect.Slice:
		if values, ok := node.([]any); ok && value.Type().Elem().Kind() != reflect.Uint8 {
			for i := range values {
				values[i] = widenJSONFloats(value.Index(i), values[i])
			}
		}
	}
	return node
}

func decodeField(name string, raw json.RawMessage, field reflect.Value) error {
	target := field.Addr().Interface()
	if bytes.Equal(raw, []byte("null")) {
		// A pointer models Option<T>; RawMessage models a JSON value.
		if _, arbitraryJSON := target.(*json.RawMessage); field.Kind() != reflect.Pointer && !arbitraryJSON {
			return fmt.Errorf("config field %q cannot be null", name)
		}
	}
	if stringsField, ok := target.(*[]string); ok {
		// encoding/json decodes null entries in a []string as empty strings.
		var entries []*string
		if err := json.Unmarshal(raw, &entries); err != nil {
			return fmt.Errorf("config field %q: %w", name, err)
		}
		values := make([]string, len(entries))
		for i, entry := range entries {
			if entry == nil {
				return fmt.Errorf("config field %q entry %d cannot be null", name, i)
			}
			values[i] = *entry
		}
		*stringsField = values
		return nil
	}
	if err := json.Unmarshal(raw, target); err != nil {
		return fmt.Errorf("config field %q: %w", name, err)
	}
	return nil
}

// Rust Vec fields always serialize as arrays, including their empty value.
func nonNilSlice[T any](values []T) []T {
	if values == nil {
		return []T{}
	}
	return values
}
