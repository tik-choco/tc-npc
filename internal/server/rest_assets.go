package server

import (
	"fmt"
	"net/http"
	"os"
	"strings"

	"github.com/tik-choco/tc-npc/internal/sound"
	"github.com/tik-choco/tc-npc/internal/sprite"
	"github.com/tik-choco/tc-npc/internal/vrm"
)

const vrmFileCacheControl = "private, no-cache"

func libraryErrorStatus(err error) int {
	for _, reason := range []string{"invalid vrm file name", "not a .vrm file", "invalid sprite file name", "not a .png file"} {
		if strings.Contains(err.Error(), reason) {
			return http.StatusBadRequest
		}
	}
	return http.StatusInternalServerError
}

func (s *appState) apiListVRM(w http.ResponseWriter, _ *http.Request) {
	models, err := vrm.ListVRMModels(s.mc.DataDir)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, object{
		"models": models, "dir": vrm.VRMDir(s.mc.DataDir), "default": s.current.Load().Character.AvatarFile,
	})
}

func vrmETag(stat *vrm.FileStat) string {
	seconds, nanos := stat.Modified.Unix(), stat.Modified.Nanosecond()
	if seconds < 0 {
		seconds, nanos = 0, 0
	}
	return fmt.Sprintf("\"%x-%x-%x\"", stat.Size, seconds, nanos)
}

func ifNoneMatchCovers(value, etag string) bool {
	if strings.TrimSpace(value) == "*" {
		return true
	}
	for part := range strings.SplitSeq(value, ",") {
		candidate := strings.TrimSpace(part)
		for strings.HasPrefix(candidate, "W/") {
			candidate = candidate[2:]
		}
		if candidate == etag {
			return true
		}
	}
	return false
}

func (s *appState) apiGetVRMFile(w http.ResponseWriter, r *http.Request) {
	file := r.PathValue("file")
	// Synchronize stat and read with server-side replacements so the
	// validator and body describe the same upload.
	s.libraryMu.Lock()
	stat, err := vrm.VRMFileStat(s.mc.DataDir, file)
	if err != nil || stat == nil {
		s.libraryMu.Unlock()
		writeLibraryFile(w, nil, err, "vrm", file, "", "")
		return
	}
	etag := vrmETag(stat)
	if ifNoneMatchCovers(r.Header.Get("If-None-Match"), etag) {
		s.libraryMu.Unlock()
		// Cache hits need only metadata, avoiding a full model read.
		w.Header().Set("Cache-Control", vrmFileCacheControl)
		w.Header().Set("ETag", etag)
		w.WriteHeader(http.StatusNotModified)
		return
	}
	data, err := vrm.ReadVRM(s.mc.DataDir, file)
	s.libraryMu.Unlock()
	writeLibraryFile(w, data, err, "vrm", file, "model/gltf-binary", etag)
}

func (s *appState) apiUploadVRM(w http.ResponseWriter, r *http.Request) {
	body, ok := readUpload(w, r, maxVRMUploadBytes)
	if !ok {
		return
	}
	s.libraryMu.Lock()
	defer s.libraryMu.Unlock()
	model, err := vrm.SaveVRM(s.mc.DataDir, r.PathValue("file"), body)
	s.finishLibraryUpload(w, model, err)
}

func (s *appState) apiDeleteVRM(w http.ResponseWriter, r *http.Request) {
	file := r.PathValue("file")
	s.libraryMu.Lock()
	defer s.libraryMu.Unlock()
	deleted, err := vrm.DeleteVRM(s.mc.DataDir, file)
	s.finishLibraryDelete(w, deleted, err, "vrm", file)
}

func (s *appState) apiListSprites(w http.ResponseWriter, _ *http.Request) {
	sheets, err := sprite.ListSprites(s.mc.DataDir)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, object{"sheets": sheets, "dir": sprite.SpriteDir(s.mc.DataDir)})
}

func (s *appState) apiGetSpriteFile(w http.ResponseWriter, r *http.Request) {
	file := r.PathValue("file")
	s.libraryMu.Lock()
	data, err := sprite.ReadSprite(s.mc.DataDir, file)
	s.libraryMu.Unlock()
	// Sheets are much smaller than models and need only no-cache.
	writeLibraryFile(w, data, err, "sprite", file, "image/png", "")
}

func (s *appState) apiUploadSprite(w http.ResponseWriter, r *http.Request) {
	body, ok := readUpload(w, r, maxSpriteUploadBytes)
	if !ok {
		return
	}
	s.libraryMu.Lock()
	defer s.libraryMu.Unlock()
	sheet, err := sprite.SaveSprite(s.mc.DataDir, r.PathValue("file"), body)
	s.finishLibraryUpload(w, sheet, err)
}

func (s *appState) apiDeleteSprite(w http.ResponseWriter, r *http.Request) {
	file := r.PathValue("file")
	s.libraryMu.Lock()
	defer s.libraryMu.Unlock()
	deleted, err := sprite.DeleteSprite(s.mc.DataDir, file)
	s.finishLibraryDelete(w, deleted, err, "sprite", file)
}

func readUpload(w http.ResponseWriter, r *http.Request, limit int64) ([]byte, bool) {
	body, ok := readBody(w, r, limit, false)
	if ok && len(body) == 0 {
		writeError(w, http.StatusBadRequest, "empty request body")
		return nil, false
	}
	return body, ok
}

func writeLibraryFile(w http.ResponseWriter, data []byte, err error, kind, file, contentType, etag string) {
	if err != nil {
		writeError(w, libraryErrorStatus(err), err.Error())
		return
	}
	if data == nil {
		writeError(w, http.StatusNotFound, kind+" not found: "+file)
		return
	}
	w.Header().Set("Cache-Control", vrmFileCacheControl)
	if etag != "" {
		w.Header().Set("ETag", etag)
	}
	writeBytes(w, contentType, data)
}

func (s *appState) finishLibraryUpload(w http.ResponseWriter, entry any, err error) {
	if err != nil {
		writeError(w, libraryErrorStatus(err), err.Error())
		return
	}
	// Adding the file may resolve an existing, dangling assignment.
	s.broadcastAvatar()
	writeJSON(w, entry)
}

func (s *appState) finishLibraryDelete(w http.ResponseWriter, deleted bool, err error, kind, file string) {
	if err != nil {
		writeError(w, libraryErrorStatus(err), err.Error())
		return
	}
	if !deleted {
		writeError(w, http.StatusNotFound, kind+" not found: "+file)
		return
	}
	// References survive deletion, so restoring the file restores the avatar.
	s.broadcastAvatar()
	writeJSON(w, object{"ok": true})
}

func (s *appState) apiListSound(w http.ResponseWriter, _ *http.Request) {
	sounds, err := sound.ListSounds(s.mc.DataDir)
	if err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, object{"sounds": sounds, "dir": sound.SoundDir(s.mc.DataDir)})
}

func (s *appState) apiRevealSoundFolder(w http.ResponseWriter, r *http.Request) {
	dir := sound.SoundDir(s.mc.DataDir)
	if err := os.MkdirAll(dir, 0o777); err != nil {
		writeError(w, http.StatusInternalServerError, fmt.Sprintf("failed to create %s: %v", dir, err))
		return
	}
	if err := openTarget(r.Context(), dir); err != nil {
		writeError(w, http.StatusInternalServerError, fmt.Sprintf("failed to open %s: %v", dir, err))
		return
	}
	writeJSON(w, object{"ok": true, "dir": dir})
}
