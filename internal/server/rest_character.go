package server

import (
	"fmt"
	"net/http"
	"strings"

	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/sprite"
	"github.com/tik-choco/tc-npc/internal/vrm"
)

type characterRef struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

func activeCharacter(s *appState, id string) *character.Character {
	c, _ := character.ActiveCharacter(s.mc.DataDir, id)
	return c
}

func characterJSON(c *character.Character) object {
	if c == nil {
		return nil
	}
	return object{"id": c.ID, "name": c.Sheet.Name, "avatar": c.Avatar}
}

func (s *appState) apiState(w http.ResponseWriter, _ *http.Request) {
	cfg := s.current.Load()
	c := activeCharacter(s, cfg.Character.ActiveID)
	writeJSON(w, object{
		"modules": flagsFor(cfg), "character": characterJSON(c),
		"avatar": s.resolveAvatar(c, cfg.Character.AvatarFile), "addr": s.addr,
	})
}

func (s *appState) apiListCharacters(w http.ResponseWriter, _ *http.Request) {
	characters, err := character.ListCharacters(s.mc.DataDir)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	active := s.current.Load().Character.ActiveID
	out := make([]object, 0, len(characters))
	for i := range characters {
		view := characterJSON(&characters[i])
		view["active"] = characters[i].ID == active
		out = append(out, view)
	}
	writeJSON(w, out)
}

func (s *appState) apiImportCharacters(w http.ResponseWriter, r *http.Request) {
	body, ok := readBody(w, r, defaultBodyLimit, true)
	if !ok {
		return
	}

	s.libraryMu.Lock()
	defer s.libraryMu.Unlock()
	characters, err := character.ImportTCTownExportInto(string(body), s.mc.DataDir)
	if err != nil {
		writeError(w, http.StatusBadRequest, err.Error())
		return
	}
	out := make([]characterRef, 0, len(characters))
	for i := range characters {
		c := &characters[i]
		if err := character.SaveCharacter(s.mc.DataDir, c); err != nil {
			writeError(w, http.StatusInternalServerError, fmt.Sprintf("failed to save character %s: %v", c.ID, err))
			return
		}
		out = append(out, characterRef{ID: c.ID, Name: c.Sheet.Name})
	}
	writeJSON(w, out)
}

func (s *appState) apiActivateCharacter(w http.ResponseWriter, r *http.Request) {
	id := r.PathValue("id")
	s.configMu.Lock()
	defer s.configMu.Unlock()
	s.libraryMu.Lock()
	defer s.libraryMu.Unlock()
	if err := s.editConfigFile(func(doc object) { setConfigField(doc, "character", "active_id", id) }); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	updated := *s.current.Load()
	updated.Character.ActiveID = id
	s.publishConfig(&updated)
	s.broadcastAvatar()
	writeJSON(w, object{"ok": true, "note": "persona and avatar switched immediately; no restart required"})
}

type setDefaultAvatarRequest struct {
	File *string `json:"file"`
}

func (req *setDefaultAvatarRequest) UnmarshalJSON(data []byte) error {
	return decodeFields(data, req, true)
}

func (s *appState) apiSetDefaultAvatar(w http.ResponseWriter, r *http.Request) {
	var req setDefaultAvatarRequest
	if !readJSONRequest(w, r, "invalid JSON: ", &req) {
		return
	}
	file := strings.TrimSpace(valueOr(req.File, ""))
	s.configMu.Lock()
	defer s.configMu.Unlock()
	s.libraryMu.Lock()
	defer s.libraryMu.Unlock()
	if file != "" && !s.checkAvatarFile(w, "vrm", file) {
		return
	}
	if err := s.editConfigFile(func(doc object) { setConfigField(doc, "character", "avatar_file", file) }); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	updated := *s.current.Load()
	updated.Character.AvatarFile = file
	s.current.Store(&updated)
	s.broadcastAvatar()
	writeJSON(w, object{"ok": true, "file": file})
}

type setAvatarRequest struct {
	File *string `json:"file"`
	Kind *string `json:"kind"`
}

func (req *setAvatarRequest) UnmarshalJSON(data []byte) error {
	return decodeFields(data, req, true)
}

func (s *appState) apiSetCharacterAvatar(w http.ResponseWriter, r *http.Request) {
	var req setAvatarRequest
	if !readJSONRequest(w, r, "invalid JSON: ", &req) {
		return
	}
	s.libraryMu.Lock()
	defer s.libraryMu.Unlock()
	c, err := character.LoadCharacter(s.mc.DataDir, r.PathValue("id"))
	if err != nil {
		writeError(w, http.StatusNotFound, err.Error())
		return
	}
	c.Avatar = nil
	file := strings.TrimSpace(valueOr(req.File, ""))
	if file != "" {
		kind := "vrm"
		if valueOr(req.Kind, "") == "sprite" {
			kind = "sprite"
		}
		if !s.checkAvatarFile(w, kind, file) {
			return
		}
		c.Avatar = &character.Avatar{Kind: kind, File: file}
	}
	if err := character.SaveCharacter(s.mc.DataDir, &c); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	s.broadcastAvatar()
	writeJSON(w, characterJSON(&c))
}

func (s *appState) checkAvatarFile(w http.ResponseWriter, kind, file string) bool {
	existsIn := vrm.VRMExists
	if kind == "sprite" {
		existsIn = sprite.SpriteExists
	}
	exists, err := existsIn(s.mc.DataDir, file)
	switch {
	case err != nil:
		writeError(w, libraryErrorStatus(err), err.Error())
	case !exists:
		writeError(w, http.StatusNotFound, kind+" not found: "+file)
	default:
		return true
	}
	return false
}
