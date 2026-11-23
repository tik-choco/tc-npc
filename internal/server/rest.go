package server

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"unicode/utf8"

	"github.com/tik-choco/tc-npc/internal/audio"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
)

type object = map[string]any

func healthz(w http.ResponseWriter, _ *http.Request) {
	writeJSON(w, object{"ok": true})
}

func writeJSON(w http.ResponseWriter, value any) {
	writeJSONStatus(w, http.StatusOK, value)
}

func writeJSONStatus(w http.ResponseWriter, status int, value any) {
	var data bytes.Buffer
	enc := json.NewEncoder(&data)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(value); err != nil {
		writeText(w, http.StatusInternalServerError, err.Error())
		return
	}
	body := bytes.TrimSuffix(data.Bytes(), []byte{10})
	w.Header().Set("Content-Type", "application/json")
	w.Header().Set("Content-Length", strconv.Itoa(len(body)))
	w.WriteHeader(status)
	_, _ = w.Write(body)
}

func writeError(w http.ResponseWriter, status int, message string) {
	writeJSONStatus(w, status, object{"error": message})
}

func writeText(w http.ResponseWriter, status int, message string) {
	w.Header().Set("Content-Type", "text/plain; charset=utf-8")
	w.Header().Set("Content-Length", strconv.Itoa(len(message)))
	w.WriteHeader(status)
	_, _ = io.WriteString(w, message)
}

func writeBytes(w http.ResponseWriter, contentType string, data []byte) {
	w.Header().Set("Content-Type", contentType)
	w.Header().Set("Content-Length", strconv.Itoa(len(data)))
	_, _ = w.Write(data)
}

func readBody(w http.ResponseWriter, r *http.Request, limit int64, text bool) ([]byte, bool) {
	const tooLargeMessage = "Failed to buffer the request body: length limit exceeded"
	if r.ContentLength > limit {
		writeText(w, http.StatusRequestEntityTooLarge, tooLargeMessage)
		return nil, false
	}
	body, err := io.ReadAll(http.MaxBytesReader(w, r.Body, limit))
	var tooLarge *http.MaxBytesError
	switch {
	case errors.As(err, &tooLarge):
		writeText(w, http.StatusRequestEntityTooLarge, tooLargeMessage)
	case err != nil:
		writeText(w, http.StatusBadRequest, "Failed to buffer the request body: "+err.Error())
	case text && !utf8.Valid(body):
		for i := 0; i < len(body); {
			char, size := utf8.DecodeRune(body[i:])
			if char == utf8.RuneError && size == 1 {
				detail := fmt.Sprintf("invalid utf-8 sequence of %d bytes from index %d", invalidUTF8Length(body[i:]), i)
				if !utf8.FullRune(body[i:]) {
					detail = fmt.Sprintf("incomplete utf-8 byte sequence from index %d", i)
				}
				writeText(w, http.StatusBadRequest, "Request body didn't contain valid UTF-8: "+detail)
				break
			}
			i += size
		}
	default:
		return body, true
	}
	return nil, false
}

func invalidUTF8Length(data []byte) int {
	if len(data) < 2 || data[0] < 0xc2 || data[0] > 0xf4 {
		return 1
	}
	first, second := data[0], data[1]
	if second < 0x80 || second > 0xbf || first < 0xe0 ||
		first == 0xe0 && second < 0xa0 || first == 0xed && second >= 0xa0 ||
		first == 0xf0 && second < 0x90 || first == 0xf4 && second >= 0x90 {
		return 1
	}
	if first >= 0xf0 && len(data) > 2 && data[2] >= 0x80 && data[2] <= 0xbf {
		return 3
	}
	return 2
}

func readJSONRequest(w http.ResponseWriter, r *http.Request, prefix string, target any) bool {
	body, ok := readBody(w, r, defaultBodyLimit, true)
	if !ok {
		return false
	}
	if err := json.Unmarshal(body, target); err != nil {
		writeError(w, http.StatusBadRequest, prefix+err.Error())
		return false
	}
	return true
}

func decodeValue(body []byte) (any, error) {
	if !json.Valid(body) {
		var value json.RawMessage
		return nil, json.Unmarshal(body, &value)
	}
	dec := json.NewDecoder(bytes.NewReader(body))
	dec.UseNumber()
	var value any
	err := dec.Decode(&value)
	return value, err
}

// decodeFields matches Serde's struct decoding. Unknown fields are ignored,
// known field names are case-sensitive, and only optional fields accept null.
func decodeFields(body []byte, target any, allowSequence bool) error {
	if !json.Valid(body) {
		var value json.RawMessage
		return json.Unmarshal(body, &value)
	}
	v := reflect.ValueOf(target).Elem()
	fields := make(map[string]int, v.NumField())
	for i := 0; i < v.NumField(); i++ {
		name, _, _ := strings.Cut(v.Type().Field(i).Tag.Get("json"), ",")
		fields[name] = i
	}
	dec := json.NewDecoder(bytes.NewReader(body))
	start, err := dec.Token()
	if err != nil {
		return err
	}
	sequence := start == json.Delim('[') && allowSequence
	if start != json.Delim('{') && !sequence {
		return errors.New("expected a JSON object")
	}
	seen := make([]bool, v.NumField())
	for i := 0; dec.More(); i++ {
		if sequence && i >= v.NumField() {
			return errors.New("too many fields")
		}
		index, known := i, true
		if !sequence {
			key, err := dec.Token()
			if err != nil {
				return err
			}
			index, known = fields[key.(string)]
		}
		var raw json.RawMessage
		if err := dec.Decode(&raw); err != nil {
			return err
		}
		if !known {
			continue
		}
		field := v.Field(index)
		name, _, _ := strings.Cut(v.Type().Field(index).Tag.Get("json"), ",")
		if seen[index] {
			return fmt.Errorf("duplicate field %q", name)
		}
		seen[index] = true
		if bytes.Equal(raw, []byte("null")) {
			if field.Kind() != reflect.Pointer {
				return fmt.Errorf("invalid type: null for field %q", name)
			}
			continue
		}
		if err := json.Unmarshal(raw, field.Addr().Interface()); err != nil {
			return fmt.Errorf("field %q: %w", name, err)
		}
	}
	for i, present := range seen {
		field := v.Type().Field(i)
		if !present && field.Tag.Get("required") == "true" {
			return fmt.Errorf("missing field %q", field.Tag.Get("json"))
		}
	}
	return nil
}

func (s *appState) apiGetConfig(w http.ResponseWriter, _ *http.Request) {
	doc, err := s.current.Load().RedactedJSON()
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, doc)
}

func (s *appState) apiPutConfig(w http.ResponseWriter, r *http.Request) {
	body, ok := readBody(w, r, defaultBodyLimit, true)
	if !ok {
		return
	}
	incoming, err := decodeValue(body)
	if err != nil {
		writeError(w, http.StatusBadRequest, "invalid JSON: "+err.Error())
		return
	}

	s.configMu.Lock()
	defer s.configMu.Unlock()
	current := s.current.Load()
	if doc, ok := incoming.(map[string]any); ok {
		config.UnmaskProviderKeys(doc, current)
	}
	// Provider IDs take precedence over array positions. The positional
	// pass still handles the older API/TTS/STT/Vision sections.
	serialized, err := json.Marshal(current)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	existing, err := decodeValue(serialized)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	restoreMaskedSecrets(incoming, existing)
	merged, err := json.Marshal(incoming)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	var updated config.Config
	if err := json.Unmarshal(merged, &updated); err != nil {
		writeError(w, http.StatusBadRequest, "invalid config: "+err.Error())
		return
	}
	updated.MigrateLLMConfig()
	if err := updated.Save(s.mc.ConfigPath); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	s.publishConfig(&updated)
	s.mist.submit(&updated)

	writeJSON(w, object{
		"ok":   true,
		"note": "Schedule, location/route and speech (stt/tts on-off, devices, endpoints, VAD) changes take effect immediately; other changes (API connections, module on/off, etc.) require a restart to take effect.",
	})
}

func restoreMaskedSecrets(incoming, current any) {
	switch node := incoming.(type) {
	case map[string]any:
		old, _ := current.(map[string]any)
		for key, value := range node {
			real, exists := old[key]
			if !exists {
				continue
			}
			if key == "api_key" {
				if value == "***" {
					node[key] = real
				}
				continue
			}
			restoreMaskedSecrets(value, real)
		}
	case []any:
		old, _ := current.([]any)
		for i := 0; i < min(len(node), len(old)); i++ {
			restoreMaskedSecrets(node[i], old[i])
		}
	}
}

func (s *appState) publishConfig(updated *config.Config) {
	s.mc.Bus.Publish(bus.TopicConfig, bus.MsgConfigUpdated, updated)
	s.current.Store(updated)
}

// editConfigFile patches the on-disk document so activation preserves
// unknown fields as well as the known settings in the live snapshot.
// Call under configMu to serialize it with config replacement.
func (s *appState) editConfigFile(edit func(object)) error {
	path := config.ResolvePath(s.mc.ConfigPath)
	data, err := os.ReadFile(path)
	var value any
	if err == nil && utf8.Valid(data) {
		value, err = decodeValue(data)
	}
	if err != nil || !utf8.Valid(data) {
		snapshot, _ := json.Marshal(s.current.Load())
		value, _ = decodeValue(snapshot)
	}
	doc, ok := value.(map[string]any)
	if !ok {
		doc = object{}
	}
	edit(doc)
	pretty, err := json.MarshalIndent(doc, "", "  ")
	if err != nil {
		return err
	}
	if err := replaceConfigFile(path, pretty); err != nil {
		return fmt.Errorf("failed to write %s: %w", path, err)
	}
	return nil
}

func replaceConfigFile(path string, data []byte) error {
	mode := os.FileMode(0o600)
	info, err := os.Lstat(path)
	if err == nil && info.Mode()&os.ModeSymlink != 0 {
		path, err = filepath.EvalSymlinks(path)
		if err != nil {
			return err
		}
		info, err = os.Stat(path)
	}
	if err == nil {
		if !info.Mode().IsRegular() {
			return errors.New("config path is not a regular file")
		}
		mode = info.Mode().Perm()
	} else if !errors.Is(err, os.ErrNotExist) {
		return err
	}
	// Typed config saves use config.Save. This path must retain the raw
	// document, including fields from newer versions.
	tmp, err := os.CreateTemp(filepath.Dir(path), ".config-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	_, writeErr := tmp.Write(data)
	if err := errors.Join(writeErr, tmp.Chmod(mode), tmp.Close()); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), path)
}

func setConfigField(doc object, section, key string, value any) {
	child, ok := doc[section].(map[string]any)
	if !ok {
		child = object{}
		doc[section] = child
	}
	child[key] = value
}

func apiAudioDevices(w http.ResponseWriter, _ *http.Request) {
	in, _ := audio.ListInputDevices()
	out, _ := audio.ListOutputDevices()
	input, defaultInput := deviceNames(in)
	output, defaultOutput := deviceNames(out)
	writeJSON(w, object{
		"input": input, "output": output, "default_input": defaultInput, "default_output": defaultOutput,
	})
}

func deviceNames(devices []audio.DeviceInfo) ([]string, *string) {
	names := make([]string, 0, len(devices))
	seen := make(map[string]bool, len(devices))
	var defaultName *string
	for _, device := range devices {
		if strings.TrimSpace(device.Name) == "" {
			continue
		}
		if defaultName == nil && device.IsDefault {
			defaultName = &device.Name
		}
		if !seen[device.Name] {
			seen[device.Name] = true
			names = append(names, device.Name)
		}
	}
	return names, defaultName
}
