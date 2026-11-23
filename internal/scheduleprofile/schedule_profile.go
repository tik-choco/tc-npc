// Package scheduleprofile stores named snapshots of the live scheduler config.
// Activating a profile copies its schedule into config.scheduler, which the
// clock reads directly.
package scheduleprofile

import (
	"bytes"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/tik-choco/tc-npc/internal/config"
)

// ScheduleProfile is a saved, named scheduler snapshot.
type ScheduleProfile struct {
	ID        string                 `json:"id"`
	Label     string                 `json:"label"`
	CreatedAt string                 `json:"created_at"`
	UpdatedAt string                 `json:"updated_at"`
	Scheduler config.SchedulerConfig `json:"scheduler"`
}

// UnmarshalJSON applies the Rust defaults and requires an id.
func (p *ScheduleProfile) UnmarshalJSON(data []byte) error {
	if !utf8.Valid(data) {
		return errors.New("schedule profile JSON contains invalid UTF-8")
	}
	profile := ScheduleProfile{Scheduler: config.DefaultSchedulerConfig()}
	names := [...]string{"id", "label", "created_at", "updated_at", "scheduler"}
	fields := [...]any{&profile.ID, &profile.Label, &profile.CreatedAt, &profile.UpdatedAt, &profile.Scheduler}
	seen := make(map[string]bool, len(fields))
	decode := func(name string, raw json.RawMessage, i int) error {
		if seen[name] {
			return fmt.Errorf("duplicate schedule profile field %q", name)
		}
		if bytes.Equal(raw, []byte("null")) {
			return fmt.Errorf("schedule profile field %q cannot be null", name)
		}
		if err := json.Unmarshal(raw, fields[i]); err != nil {
			return fmt.Errorf("schedule profile field %q: %w", name, err)
		}
		seen[name] = true
		return nil
	}

	decoder := json.NewDecoder(bytes.NewReader(data))
	start, err := decoder.Token()
	if err != nil {
		return err
	}
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
			for i, name := range names {
				if key == name {
					if err := decode(name, raw, i); err != nil {
						return err
					}
					break
				}
			}
		}
	case json.Delim('['):
		// Serde also accepts structs as positional sequences.
		for i := 0; decoder.More(); i++ {
			if i >= len(fields) {
				return errors.New("too many schedule profile fields")
			}
			var raw json.RawMessage
			if err := decoder.Decode(&raw); err != nil {
				return err
			}
			if err := decode(names[i], raw, i); err != nil {
				return err
			}
		}
	default:
		return errors.New("expected a JSON object or array for schedule profile")
	}
	if _, err := decoder.Token(); err != nil {
		return err
	}
	if _, err := decoder.Token(); err != io.EOF {
		return errors.New("unexpected data after schedule profile")
	}
	if !seen["id"] {
		return errors.New("missing schedule profile field \"id\"")
	}
	*p = profile
	return nil
}

func scheduleProfilesDir(dataDir string) string {
	return filepath.Join(dataDir, "schedule_profiles")
}

func scheduleProfilePath(dataDir, id string) string {
	return filepath.Join(scheduleProfilesDir(dataDir), id+".json")
}

// IDs can come straight from HTTP path segments. Check both separators and
// drive prefixes so the id cannot escape the profiles folder.
func validateScheduleProfileID(id string) error {
	if id == "" || strings.ContainsAny(id, `/\:`) || strings.Contains(id, "..") || strings.HasPrefix(id, ".") {
		return fmt.Errorf("invalid schedule profile id: %q", id)
	}
	return nil
}

// ListScheduleProfiles returns the saved profiles sorted by id.
// A missing directory yields an empty list. Malformed JSON files are skipped.
func ListScheduleProfiles(dataDir string) ([]ScheduleProfile, error) {
	dir := scheduleProfilesDir(dataDir)
	profiles := []ScheduleProfile{}
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return profiles, nil
	}
	if err != nil {
		return nil, err
	}
	for _, entry := range entries {
		name := entry.Name()
		if name == ".json" || filepath.Ext(name) != ".json" {
			continue
		}
		path := filepath.Join(dir, name)
		data, err := readProfile(path)
		if err != nil {
			return nil, err
		}
		var profile ScheduleProfile
		if err := json.Unmarshal(data, &profile); err != nil {
			slog.Warn("skipping unreadable schedule profile file", "path", path, "error", err)
			continue
		}
		profiles = append(profiles, profile)
	}
	sort.SliceStable(profiles, func(i, j int) bool { return profiles[i].ID < profiles[j].ID })
	return profiles, nil
}

// SaveScheduleProfile creates or overwrites a profile, creating its directory
// if needed.
func SaveScheduleProfile(dataDir string, profile *ScheduleProfile) error {
	if err := validateScheduleProfileID(profile.ID); err != nil {
		return err
	}
	var data bytes.Buffer
	encoder := json.NewEncoder(&data)
	encoder.SetEscapeHTML(false)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(profile); err != nil {
		return fmt.Errorf("failed to encode schedule profile %s: %w", profile.ID, err)
	}
	dir := scheduleProfilesDir(dataDir)
	if err := os.MkdirAll(dir, 0o777); err != nil {
		return err
	}
	// Publish the completed file so readers and other saves never see partial JSON.
	tmp, err := os.CreateTemp(dir, ".schedule-profile-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	_, writeErr := tmp.Write(bytes.TrimSuffix(data.Bytes(), []byte("\n")))
	if err := errors.Join(writeErr, tmp.Close()); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), scheduleProfilePath(dataDir, profile.ID))
}

// LoadScheduleProfile reads a single profile by id.
func LoadScheduleProfile(dataDir, id string) (ScheduleProfile, error) {
	if err := validateScheduleProfileID(id); err != nil {
		return ScheduleProfile{}, err
	}
	data, err := readProfile(scheduleProfilePath(dataDir, id))
	if err != nil {
		return ScheduleProfile{}, fmt.Errorf("failed to read schedule profile %s: %w", id, err)
	}
	var profile ScheduleProfile
	if err := json.Unmarshal(data, &profile); err != nil {
		return ScheduleProfile{}, fmt.Errorf("failed to parse schedule profile %s: %w", id, err)
	}
	return profile, nil
}

// DeleteScheduleProfile removes a profile file. A missing file is fine.
func DeleteScheduleProfile(dataDir, id string) error {
	if err := validateScheduleProfileID(id); err != nil {
		return err
	}
	path := scheduleProfilePath(dataDir, id)
	info, err := os.Lstat(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if info.IsDir() {
		return fmt.Errorf("schedule profile path is a directory: %s", path)
	}
	if err := os.Remove(path); !errors.Is(err, os.ErrNotExist) {
		return err
	}
	return nil
}

func readProfile(path string) ([]byte, error) {
	data, err := os.ReadFile(path)
	if err == nil && !utf8.Valid(data) {
		return nil, fmt.Errorf("%s contains invalid UTF-8", path)
	}
	return data, err
}
