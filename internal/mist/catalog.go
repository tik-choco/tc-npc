package mist

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"strings"
	"unicode/utf8"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/module"
)

// The public catalog room joined by tc-town's web client.
const catalogRoomID = "tc-town-character-catalog-v1"

func catalogCID(data []byte) (string, bool) {
	var fields map[string]json.RawMessage
	if !utf8.Valid(data) || json.Unmarshal(data, &fields) != nil {
		return "", false
	}
	var kind string
	if json.Unmarshal(fields["type"], &kind) != nil || kind != "tc-town:catalog-entry" {
		return "", false
	}
	var cid *string
	if json.Unmarshal(fields["cid"], &cid) != nil || cid == nil {
		slog.Debug("mist: catalog entry wire missing cid, ignoring")
		return "", false
	}
	// Like npc-mist v1, discovery does not verify the entry's did:key
	// signature. Discovered sheets require an operator to import them.
	return *cid, true
}

func fetchCatalog(ctx context.Context, slots chan struct{}, e engine, mc module.Ctx, cid string) {
	select {
	case <-ctx.Done():
		return
	case slots <- struct{}{}:
	}
	defer func() { <-slots }()
	data, err := e.storageGet(ctx, cid)
	if ctx.Err() != nil {
		return
	}
	if err != nil {
		slog.Warn("mist: failed to fetch catalog payload", "cid", cid, "error", err)
		return
	}
	record, err := decodeCatalog(data)
	if err != nil {
		slog.Warn("mist: ignoring catalog payload", "cid", cid, "error", err)
		return
	}
	if err := saveDiscovered(mc.DataDir, &record); err != nil {
		slog.Warn("mist: failed to save discovered catalog character", "id", record.ID, "error", err)
		return
	}
	name := record.Sheet.Name
	if strings.TrimSpace(name) == "" {
		name = "(名称未設定)"
	}
	slog.Info("mist: catalog character discovered", "id", record.ID, "name", name)
	mc.Bus.Publish(bus.TopicUI, "action_log", map[string]string{"text": "mist: キャラクター「" + name + "」を発見 (catalog)"})
}

func decodeCatalog(data []byte) (character.Character, error) {
	var record character.Character
	if !utf8.Valid(data) {
		return record, errors.New("invalid UTF-8")
	}
	var fields map[string]json.RawMessage
	if err := json.Unmarshal(data, &fields); err != nil {
		return record, fmt.Errorf("invalid JSON: %w", err)
	}
	var (
		app, kind string
		version   uint64
		entry     map[string]json.RawMessage
	)
	if err := decodeFields(fields, map[string]any{
		"app": &app, "kind": &kind, "version": &version, "character": &entry,
	}); err != nil {
		return record, err
	}
	if app != "tc-town" || kind != "catalog-character" || version != 1 {
		return record, errors.New("unexpected catalog payload shape")
	}
	if entry == nil {
		return record, errors.New("missing catalog character")
	}
	// The catalog ships the same sheet fields as a tc-town export. Avatar
	// data is ignored, including avatar fields from unsigned publications.
	var sheet map[string]json.RawMessage
	if err := decodeFields(entry, map[string]any{
		"id": &record.ID, "createdAt": &record.CreatedAt, "updatedAt": &record.UpdatedAt,
		"voiceModel": &record.VoiceModel, "voiceName": &record.VoiceName, "sheet": &sheet,
	}); err != nil {
		return record, err
	}
	if _, present := entry["sheet"]; present && sheet == nil {
		return record, errors.New("expected an object for the catalog sheet")
	}
	if err := decodeFields(sheet, map[string]any{
		"name": &record.Sheet.Name, "summary": &record.Sheet.Summary, "persona": &record.Sheet.Persona,
		"speechStyle": &record.Sheet.SpeechStyle, "likes": &record.Sheet.Likes,
		"relationships": &record.Sheet.Relationships, "notes": &record.Sheet.Notes,
	}); err != nil {
		return record, err
	}
	if record.ID == "" {
		return record, errors.New("catalog character has no id")
	}
	return record, nil
}

func saveDiscovered(dataDir string, record *character.Character) error {
	name := record.ID + ".json"
	// IDs come from the network. Keep every discovery in the separate
	// catalog store even if an entry contains a path instead of an ID.
	if record.ID == "" || strings.ContainsAny(record.ID, `/\:`) || !filepath.IsLocal(name) {
		return fmt.Errorf("invalid catalog character id %q", record.ID)
	}
	dir := filepath.Join(dataDir, "catalog")
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	data, err := encodeJSON(record, "  ")
	if err != nil {
		return err
	}
	return writeAtomic(filepath.Join(dir, name), data)
}
