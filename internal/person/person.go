// Package person stores memories tied to an individual and converts them to
// the camelCase records used by the web UI.
package person

import (
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/google/uuid"
	"golang.org/x/text/cases"
	"golang.org/x/text/language"
)

// Person is a record under {dataDir}/people/{id}.json.
type Person struct {
	ID             string       `json:"id"`
	Name           string       `json:"name"`
	Aliases        []string     `json:"aliases"`
	FirstSeen      int64        `json:"first_seen"`
	LastSeen       int64        `json:"last_seen"`
	EncounterCount uint32       `json:"encounter_count"`
	Appearance     string       `json:"appearance"`
	Facts          []PersonFact `json:"facts"`
	Familiarity    float32      `json:"familiarity"`
	Source         string       `json:"source"`
	Notes          string       `json:"notes"`
}

// PersonFact is a single fact learned about a person.
type PersonFact struct {
	Text      string `json:"text"`
	Source    string `json:"source"`
	CreatedAt int64  `json:"created_at"`
}

func (p Person) MarshalJSON() ([]byte, error) {
	type record Person
	// Serde Vec fields always write arrays, including for empty records.
	p.Aliases = orEmpty(p.Aliases)
	p.Facts = orEmpty(p.Facts)
	return json.Marshal(record(p))
}

func (p *Person) UnmarshalJSON(data []byte) error {
	type record Person
	fields := struct {
		record
		ID *string `json:"id"`
	}{record: record{Aliases: []string{}, Facts: []PersonFact{}}}
	if err := json.Unmarshal(data, &fields); err != nil {
		return err
	}
	if fields.ID == nil {
		return fmt.Errorf("missing person field %q", "id")
	}
	if fields.Aliases == nil || fields.Facts == nil {
		return fmt.Errorf("person aliases and facts must be arrays")
	}
	fields.record.ID = *fields.ID
	*p = Person(fields.record)
	return nil
}

func (f *PersonFact) UnmarshalJSON(data []byte) error {
	type record PersonFact
	fields := struct {
		record
		Text *string `json:"text"`
	}{}
	if err := json.Unmarshal(data, &fields); err != nil {
		return err
	}
	if fields.Text == nil {
		return fmt.Errorf("missing person fact field %q", "text")
	}
	fields.record.Text = *fields.Text
	*f = PersonFact(fields.record)
	return nil
}

func orEmpty[T any](values []T) []T {
	if values == nil {
		return []T{}
	}
	return values
}

// NewPerson records the first encounter with a new person.
func NewPerson(name, source string) Person {
	now := time.Now().Unix()
	return Person{
		ID: uuid.NewString(), Name: name, Source: source,
		FirstSeen: now, LastSeen: now, EncounterCount: 1,
		Aliases: []string{}, Facts: []PersonFact{},
	}
}

// NormalizePersonName folds full-width spaces, strips one trailing honorific,
// trims whitespace, and lowercases for matching.
func NormalizePersonName(name string) string {
	name = strings.TrimSpace(strings.ReplaceAll(name, "\u3000", " "))
	for _, honorific := range [...]string{"せんせい", "先生", "さん", "くん", "ちゃん", "様", "氏"} {
		if trimmed, ok := strings.CutSuffix(name, honorific); ok {
			name = trimmed
			break
		}
	}
	return cases.Lower(language.Und).String(strings.TrimSpace(name))
}

// FindPerson returns the first record whose name or alias matches. An empty
// normalized name never matches another unnamed person.
func FindPerson(people []Person, name string) *Person {
	target := NormalizePersonName(name)
	if target == "" {
		return nil
	}
	for i := range people {
		p := &people[i]
		if NormalizePersonName(p.Name) == target {
			return p
		}
		for _, alias := range p.Aliases {
			if NormalizePersonName(alias) == target {
				return p
			}
		}
	}
	return nil
}

// PersonToWire returns the camelCase PersonRecord used by REST and WebSocket.
func PersonToWire(p *Person) map[string]any {
	facts := make([]map[string]any, 0, len(p.Facts))
	for _, f := range p.Facts {
		facts = append(facts, map[string]any{"text": f.Text, "source": f.Source, "createdAt": f.CreatedAt})
	}
	return map[string]any{
		"id": p.ID, "name": p.Name, "aliases": orEmpty(p.Aliases),
		"firstSeen": p.FirstSeen, "lastSeen": p.LastSeen, "encounterCount": p.EncounterCount,
		"appearance": p.Appearance, "facts": facts, "familiarity": float64(p.Familiarity),
		"source": p.Source, "notes": p.Notes,
	}
}

// PeopleDir returns the person record folder.
func PeopleDir(dataDir string) string {
	return filepath.Join(dataDir, "people")
}

func validatePersonID(id string) error {
	if id == "" || strings.IndexFunc(id, func(c rune) bool {
		return c != '-' && !(c >= '0' && c <= '9') && !(c >= 'A' && c <= 'Z') && !(c >= 'a' && c <= 'z')
	}) >= 0 {
		return fmt.Errorf("invalid person id: %q", id)
	}
	return nil
}

// ListPeople returns records sorted by id, skipping files with corrupt JSON.
func ListPeople(dataDir string) ([]Person, error) {
	dir := PeopleDir(dataDir)
	out := []Person{}
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return out, nil
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
		data, err := readRecord(path)
		if err != nil {
			return nil, err
		}
		var p Person
		if err := json.Unmarshal(data, &p); err != nil {
			slog.Warn("skipping unreadable person file", "path", path, "error", err)
			continue
		}
		out = append(out, p)
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out, nil
}

// LoadPerson returns the requested record, or nil if it is missing.
func LoadPerson(dataDir, id string) (*Person, error) {
	if err := validatePersonID(id); err != nil {
		return nil, err
	}
	path := filepath.Join(PeopleDir(dataDir), id+".json")
	if _, err := os.Stat(path); err != nil {
		return nil, nil
	}
	data, err := readRecord(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read person %s: %w", id, err)
	}
	var p Person
	if err := json.Unmarshal(data, &p); err != nil {
		return nil, fmt.Errorf("failed to parse person %s: %w", id, err)
	}
	return &p, nil
}

func readRecord(path string) ([]byte, error) {
	data, err := os.ReadFile(path)
	if err == nil && !utf8.Valid(data) {
		return nil, fmt.Errorf("%s contains invalid UTF-8", path)
	}
	return data, err
}

// SavePerson writes the full record to a temporary sibling before replacing
// the destination. Unique temp names keep concurrent saves independent.
func SavePerson(dataDir string, p *Person) error {
	if err := validatePersonID(p.ID); err != nil {
		return err
	}
	data, err := json.MarshalIndent(p, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to encode person %s: %w", p.ID, err)
	}
	dir := PeopleDir(dataDir)
	if err := os.MkdirAll(dir, 0o777); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, ".person-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	_, writeErr := tmp.Write(data)
	if err := errors.Join(writeErr, tmp.Close()); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), filepath.Join(dir, p.ID+".json"))
}

// DeletePerson removes a record and reports whether it existed.
func DeletePerson(dataDir, id string) (bool, error) {
	if err := validatePersonID(id); err != nil {
		return false, err
	}
	path := filepath.Join(PeopleDir(dataDir), id+".json")
	if _, err := os.Stat(path); err != nil {
		return false, nil
	}
	info, err := os.Lstat(path)
	if err != nil {
		return false, err
	}
	if info.IsDir() {
		return false, fmt.Errorf("failed to delete person %s: is a directory", id)
	}
	if err := os.Remove(path); err != nil {
		return false, err
	}
	return true, nil
}
