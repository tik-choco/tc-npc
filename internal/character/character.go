// Package character stores persona sheets and imports tc-town exports.
package character

import (
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"unicode/utf8"

	"github.com/tik-choco/tc-npc/internal/sprite"
	"github.com/tik-choco/tc-npc/internal/vrm"
)

// CharacterSheet contains the free-text fields used in the persona prompt.
type CharacterSheet struct {
	Name          string `json:"name"`
	Summary       string `json:"summary"`
	Persona       string `json:"persona"`
	SpeechStyle   string `json:"speech_style"`
	Likes         string `json:"likes"`
	Relationships string `json:"relationships"`
	Notes         string `json:"notes"`
}

// Avatar references a file in the library selected by Kind. The reference
// can outlive the file, in which case the UI can fall back to the initial.
type Avatar struct {
	Kind string `json:"kind"`
	File string `json:"file"`
}

func (a *Avatar) UnmarshalJSON(data []byte) error {
	var fields struct {
		Kind *string `json:"kind"`
		File *string `json:"file"`
	}
	if err := json.Unmarshal(data, &fields); err != nil {
		return err
	}
	if fields.Kind == nil || fields.File == nil {
		return fmt.Errorf("avatar requires kind and file")
	}
	*a = Avatar{Kind: *fields.Kind, File: *fields.File}
	return nil
}

// NewVRMAvatar creates a reference to a file in the VRM library.
func NewVRMAvatar(file string) Avatar {
	return Avatar{Kind: "vrm", File: file}
}

// Character is a stored sheet with identity, timestamps, optional voice
// selection, and an optional avatar.
type Character struct {
	ID         string         `json:"id"`
	CreatedAt  string         `json:"created_at"`
	UpdatedAt  string         `json:"updated_at"`
	Sheet      CharacterSheet `json:"sheet"`
	VoiceModel *string        `json:"voice_model"`
	VoiceName  *string        `json:"voice_name"`
	Avatar     *Avatar        `json:"avatar"`
}

func (c *Character) UnmarshalJSON(data []byte) error {
	type record Character
	fields := struct {
		record
		ID *string `json:"id"`
	}{}
	if err := json.Unmarshal(data, &fields); err != nil {
		return err
	}
	if fields.ID == nil {
		return fmt.Errorf("missing character field %q", "id")
	}
	fields.record.ID = *fields.ID
	*c = Character(fields.record)
	return nil
}

type tcTownExport struct {
	App        string            `json:"app"`
	Version    uint64            `json:"version"`
	Kind       string            `json:"kind"`
	Characters []tcTownCharacter `json:"characters"`
}

type tcTownCharacter struct {
	ID         string        `json:"id"`
	CreatedAt  string        `json:"createdAt"`
	UpdatedAt  string        `json:"updatedAt"`
	Sheet      tcTownSheet   `json:"sheet"`
	VoiceModel *string       `json:"voiceModel"`
	VoiceName  *string       `json:"voiceName"`
	Avatar     *tcTownAvatar `json:"avatar"`
}

type tcTownSheet struct {
	Name          string `json:"name"`
	Summary       string `json:"summary"`
	Persona       string `json:"persona"`
	SpeechStyle   string `json:"speechStyle"`
	Likes         string `json:"likes"`
	Relationships string `json:"relationships"`
	Notes         string `json:"notes"`
}

type tcTownAvatar struct {
	Kind     string `json:"kind"`
	FileName string `json:"fileName"`
	DataURL  string `json:"dataUrl"`
}

func (c tcTownCharacter) localCharacter() Character {
	local := Character{
		ID: c.ID, CreatedAt: c.CreatedAt, UpdatedAt: c.UpdatedAt,
		Sheet: CharacterSheet(c.Sheet), VoiceModel: c.VoiceModel, VoiceName: c.VoiceName,
	}
	if c.Avatar != nil && c.Avatar.Kind == "vrm" {
		// tc-town's browser library is separate, but the original model
		// name can resolve to a file of the same name in the local library.
		file := strings.TrimSpace(c.Avatar.FileName)
		if vrm.ValidateVRMFileName(file) == nil {
			avatar := NewVRMAvatar(file)
			local.Avatar = &avatar
		}
	}
	return local
}

// ImportTCTownExport parses a tc-town bundle. Local VRM references are kept.
func ImportTCTownExport(jsonText string) ([]Character, error) {
	return importTCTownExport(jsonText, nil)
}

// ImportTCTownExportInto additionally materializes embedded image avatars
// in the sprite library. A failed image leaves the imported sheet usable.
func ImportTCTownExportInto(jsonText, dataDir string) ([]Character, error) {
	return importTCTownExport(jsonText, &dataDir)
}

func importTCTownExport(jsonText string, dataDir *string) ([]Character, error) {
	if !utf8.ValidString(jsonText) {
		return nil, fmt.Errorf("failed to parse tc-town export: invalid UTF-8")
	}
	var export *tcTownExport
	if err := json.Unmarshal([]byte(jsonText), &export); err != nil {
		return nil, fmt.Errorf("failed to parse tc-town export: %w", err)
	}
	if export == nil {
		return nil, fmt.Errorf("failed to parse tc-town export: expected an object")
	}
	out := make([]Character, 0, len(export.Characters))
	for _, item := range export.Characters {
		c := item.localCharacter()
		if dataDir != nil && item.Avatar != nil && item.Avatar.Kind == "image" {
			if dataURL := strings.TrimSpace(item.Avatar.DataURL); dataURL != "" {
				avatar, err := materializeImageAvatar(*dataDir, c.ID, dataURL)
				if err != nil {
					slog.Warn("npc-core: could not build a sprite sheet from the imported image avatar; importing the character without one", "character", c.ID, "error", err)
				} else {
					c.Avatar = avatar
				}
			}
		}
		out = append(out, c)
	}
	return out, nil
}

func materializeImageAvatar(dataDir, characterID, dataURL string) (*Avatar, error) {
	// Name the sheet after the character so re-imports replace its sheet.
	// SaveSprite validates the generated filename before it touches disk.
	file := characterID + ".png"
	data, err := sprite.SheetFromImageDataURL(dataURL)
	if err != nil {
		return nil, err
	}
	if _, err := sprite.SaveSprite(dataDir, file, data); err != nil {
		return nil, err
	}
	return &Avatar{Kind: "sprite", File: file}, nil
}

// PersonaPrompt renders the Japanese persona prompt used by tc-town.
func PersonaPrompt(sheet *CharacterSheet) string {
	var out strings.Builder
	fmt.Fprintf(&out, "あなたは「%s」というキャラクターです。以下の設定に従って、そのキャラクター本人として応答してください。\n", sheet.Name)
	sections := [...]struct{ heading, body string }{
		{"# 概要", sheet.Summary},
		{"# 人物・背景", sheet.Persona},
		{"# 話し方", sheet.SpeechStyle},
		{"# 好きなもの・嫌いなもの", sheet.Likes},
		{"# 人間関係", sheet.Relationships},
		{"# メモ", sheet.Notes},
	}
	for _, section := range sections {
		if body := strings.TrimSpace(section.body); body != "" {
			fmt.Fprintf(&out, "\n%s\n%s\n", section.heading, body)
		}
	}
	return out.String()
}

func charactersDir(dataDir string) string {
	return filepath.Join(dataDir, "characters")
}

// IDs arrive directly from HTTP paths. Check both separators and prevent
// path escapes while allowing readable ids in hand-authored sheets.
func validateCharacterID(id string) error {
	if id == "" || strings.ContainsAny(id, `/\:`) || strings.Contains(id, "..") || strings.HasPrefix(id, ".") {
		return fmt.Errorf("invalid character id: %q", id)
	}
	return nil
}

// ListCharacters returns records sorted by id, skipping files with corrupt JSON.
func ListCharacters(dataDir string) ([]Character, error) {
	dir := charactersDir(dataDir)
	out := []Character{}
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
		var c Character
		if err := json.Unmarshal(data, &c); err != nil {
			slog.Warn("skipping unreadable character file", "path", path, "error", err)
			continue
		}
		out = append(out, c)
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].ID < out[j].ID })
	return out, nil
}

// SaveCharacter atomically replaces the JSON record for a character.
func SaveCharacter(dataDir string, c *Character) error {
	if err := validateCharacterID(c.ID); err != nil {
		return err
	}
	data, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return fmt.Errorf("failed to encode character %s: %w", c.ID, err)
	}
	dir := charactersDir(dataDir)
	if err := os.MkdirAll(dir, 0o777); err != nil {
		return err
	}
	tmp, err := os.CreateTemp(dir, ".character-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	_, writeErr := tmp.Write(data)
	if err := errors.Join(writeErr, tmp.Close()); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), filepath.Join(dir, c.ID+".json"))
}

// LoadCharacter reads a character by id.
func LoadCharacter(dataDir, id string) (Character, error) {
	if err := validateCharacterID(id); err != nil {
		return Character{}, err
	}
	data, err := readRecord(filepath.Join(charactersDir(dataDir), id+".json"))
	if err != nil {
		return Character{}, fmt.Errorf("failed to read character %s: %w", id, err)
	}
	var c Character
	if err := json.Unmarshal(data, &c); err != nil {
		return Character{}, fmt.Errorf("failed to parse character %s: %w", id, err)
	}
	return c, nil
}

func readRecord(path string) ([]byte, error) {
	data, err := os.ReadFile(path)
	if err == nil && !utf8.Valid(data) {
		return nil, fmt.Errorf("%s contains invalid UTF-8", path)
	}
	return data, err
}

// ActiveCharacter returns the active character when it can be loaded.
// An empty id or unreadable sheet clears the active persona.
func ActiveCharacter(dataDir, activeID string) (*Character, error) {
	if activeID == "" {
		return nil, nil
	}
	c, err := LoadCharacter(dataDir, activeID)
	if err != nil {
		return nil, nil
	}
	return &c, nil
}
