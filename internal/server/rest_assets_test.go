package server

import (
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/audio"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/sound"
	"github.com/tik-choco/tc-npc/internal/vrm"
)

func TestVRMConditionalGETAndReplacement(t *testing.T) {
	s := testState(t)
	h := s.routes()
	const name, download = "one.vrm", "/api/vrm/file/one.vrm"
	request(t, h, http.MethodGet, download, "", http.StatusNotFound)
	request(t, h, http.MethodPost, "/api/vrm/"+name, "gltf", http.StatusOK)
	w := request(t, h, http.MethodGet, download, "", http.StatusOK)
	stat, err := vrm.VRMFileStat(s.mc.DataDir, name)
	if err != nil {
		t.Fatal(err)
	}
	etag := w.Header().Get("ETag")
	if w.Header().Get("Content-Type") != "model/gltf-binary" ||
		w.Header().Get("Cache-Control") != "private, no-cache" ||
		etag != vrmETag(stat) || w.Body.String() != "gltf" {
		t.Fatalf("VRM download: %v %s", w.Header(), w.Body)
	}

	for _, validator := range []string{etag, "*", `"old", W/` + etag} {
		r := httptest.NewRequest(http.MethodGet, download, nil)
		r.Header.Set("If-None-Match", validator)
		w := recordResponse(t, h, r, http.StatusNotModified)
		if w.Body.Len() != 0 || w.Header().Get("ETag") != etag ||
			w.Header().Get("Cache-Control") != "private, no-cache" || w.Header().Get("Content-Type") != "" {
			t.Fatalf("VRM 304: %v %q", w.Header(), w.Body)
		}
	}
	request(t, h, http.MethodPost, "/api/vrm/"+name, "new!", http.StatusOK)
	modified := stat.Modified.Add(time.Second)
	if err := os.Chtimes(filepath.Join(vrm.VRMDir(s.mc.DataDir), name), modified, modified); err != nil {
		t.Fatal(err)
	}
	r := httptest.NewRequest(http.MethodGet, download, nil)
	r.Header.Set("If-None-Match", etag)
	w = recordResponse(t, h, r, http.StatusOK)
	if w.Header().Get("ETag") == etag || w.Body.String() != "new!" {
		t.Fatalf("VRM overwrite: %v %q", w.Header(), w.Body)
	}
	list := request(t, h, http.MethodGet, "/api/vrm", "", http.StatusOK)
	out := decodeResponse[object](t, list.Body.Bytes())
	if out["default"] != "" || out["dir"] != vrm.VRMDir(s.mc.DataDir) || len(out["models"].([]any)) != 1 {
		t.Fatalf("VRM list: %v", out)
	}
	request(t, h, http.MethodDelete, "/api/vrm/"+name, "", http.StatusOK)
	request(t, h, http.MethodDelete, "/api/vrm/"+name, "", http.StatusNotFound)
	request(t, h, http.MethodGet, "/api/vrm/file/%2e%2e%2fescape.vrm", "", http.StatusBadRequest)
	request(t, h, http.MethodGet, "/api/vrm/file/%FF", "", http.StatusBadRequest)
}

func TestCharacterAssetsAndAvatarBroadcast(t *testing.T) {
	s := testState(t)
	h := s.routes()
	srv := testSocketServer(t, s)
	client := startSocket(t, srv)
	sub := s.mc.Bus.Subscribe()
	defer sub.Close()
	checkAvatar := func(kind, file string) {
		t.Helper()
		frame := readWS(t, client, "avatar")
		var want any
		if file != "" {
			want = object{"kind": kind, "file": file}
		}
		if !reflect.DeepEqual(frame["avatar"], want) {
			t.Fatalf("avatar %v, want %v", frame, want)
		}
	}

	request(t, h, http.MethodPost, "/api/vrm/default.vrm", "vrm", http.StatusOK)
	checkAvatar("", "")
	request(t, h, http.MethodPut, "/api/vrm/default", `{"file":" default.vrm "}`, http.StatusOK)
	checkAvatar("vrm", "default.vrm")
	c := dialSocket(t, srv)
	hello := readWS(t, c, "hello")
	if hello["character"] != nil || !reflect.DeepEqual(hello["avatar"], object{"kind": "vrm", "file": "default.vrm"}) {
		t.Fatalf("standalone avatar hello: %v", hello)
	}
	_ = c.CloseNow()

	const bundle = `{"app":"tc-town","version":1,"kind":"character","characters":[{"id":"npc","sheet":{"name":"Character"},"avatar":{"kind":"vrm","fileName":"missing.vrm"}}]}`
	imported := request(t, h, http.MethodPost, "/api/characters/import", bundle, http.StatusOK)
	if got := decodeResponse[[]object](t, imported.Body.Bytes()); !reflect.DeepEqual(got, []object{{"id": "npc", "name": "Character"}}) {
		t.Fatalf("imported: %v", got)
	}
	request(t, h, http.MethodPost, "/api/characters/npc/activate", "", http.StatusOK)
	if msg := receiveBus(t, sub); msg.Type != bus.MsgConfigUpdated ||
		decodeResponse[config.Config](t, msg.Payload).Character.ActiveID != "npc" {
		t.Fatalf("character activation: %+v", msg)
	}
	checkAvatar("", "")
	state := request(t, h, http.MethodGet, "/api/state", "", http.StatusOK)
	current := decodeResponse[object](t, state.Body.Bytes())
	if current["avatar"] != nil || current["character"].(map[string]any)["avatar"].(map[string]any)["file"] != "missing.vrm" {
		t.Fatalf("unresolved assigned avatar: %v", current)
	}
	request(t, h, http.MethodPost, "/api/vrm/missing.vrm", "vrm", http.StatusOK)
	checkAvatar("vrm", "missing.vrm")
	request(t, h, http.MethodDelete, "/api/vrm/missing.vrm", "", http.StatusOK)
	checkAvatar("", "")
	request(t, h, http.MethodPut, "/api/characters/npc/avatar", `{"file":"missing.vrm"}`, http.StatusNotFound)

	request(t, h, http.MethodPost, "/api/sprites/sheet.png", "png", http.StatusOK)
	checkAvatar("", "")
	request(t, h, http.MethodPut, "/api/characters/npc/avatar", `{"kind":"sprite","file":"sheet.png"}`, http.StatusOK)
	checkAvatar("sprite", "sheet.png")
	file := request(t, h, http.MethodGet, "/api/sprites/file/sheet.png", "", http.StatusOK)
	if file.Body.String() != "png" || file.Header().Get("ETag") != "" ||
		file.Header().Get("Content-Type") != "image/png" || file.Header().Get("Cache-Control") != "private, no-cache" {
		t.Fatalf("sprite download: %v %s", file.Header(), file.Body)
	}
	list := request(t, h, http.MethodGet, "/api/sprites", "", http.StatusOK)
	if got := decodeResponse[object](t, list.Body.Bytes()); len(got["sheets"].([]any)) != 1 {
		t.Fatalf("sprite list: %v", got)
	}
	request(t, h, http.MethodDelete, "/api/sprites/sheet.png", "", http.StatusOK)
	checkAvatar("", "")
	request(t, h, http.MethodPut, "/api/characters/npc/avatar", `{"file":null}`, http.StatusOK)
	checkAvatar("vrm", "default.vrm")
	request(t, h, http.MethodPut, "/api/vrm/default", `{"file":null}`, http.StatusOK)
	checkAvatar("", "")
	list = request(t, h, http.MethodGet, "/api/characters", "", http.StatusOK)
	got := decodeResponse[[]object](t, list.Body.Bytes())
	if len(got) != 1 || got[0]["active"] != true || got[0]["avatar"] != nil {
		t.Fatalf("character list: %v", got)
	}
}

func TestRequestBodyLimitsAndUTF8(t *testing.T) {
	s := testState(t)
	h := s.routes()
	const capped = "Failed to buffer the request body: length limit exceeded"
	body := "{}" + strings.Repeat(" ", defaultBodyLimit-2)
	request(t, h, http.MethodPut, "/api/config", body, http.StatusOK)
	if got := request(t, h, http.MethodPut, "/api/config", body+" ", http.StatusRequestEntityTooLarge); got.Body.String() != capped {
		t.Fatalf("request limit body: %q", got.Body)
	}
	if got := request(t, h, http.MethodPut, "/api/config", string([]byte{0xff}), http.StatusBadRequest); !strings.Contains(got.Body.String(), "invalid utf-8 sequence of 1 bytes from index 0") {
		t.Fatalf("invalid UTF-8 rejection: %q", got.Body)
	}
	size := strconv.Itoa(defaultBodyLimit + 1)
	for _, path := range []string{"/api/vrm/large.vrm", "/api/sprites/large.png"} {
		if got := request(t, h, http.MethodPost, path, body+" ", http.StatusOK); !strings.Contains(got.Body.String(), "\"size\":"+size) {
			t.Fatalf("%s upload: %s", path, got.Body)
		}
		got := request(t, h, http.MethodPost, path, "", http.StatusBadRequest)
		if got.Body.String() != `{"error":"empty request body"}` {
			t.Fatalf("%s empty upload: %s", path, got.Body)
		}
	}
	for path, limit := range map[string]int64{
		"/api/vrm/large.vrm":            maxVRMUploadBytes,
		"/api/sprites/large.png":        maxSpriteUploadBytes,
		"/api/llm/transcribe/large.wav": maxAudioUploadBytes,
	} {
		r := httptest.NewRequest(http.MethodPost, path, nil)
		r.ContentLength = limit + 1
		if got := recordResponse(t, h, r, http.StatusRequestEntityTooLarge); got.Body.String() != capped {
			t.Fatalf("%s limit rejection: %q", path, got.Body)
		}
	}
	r := httptest.NewRequest(http.MethodPut, "/api/config", strings.NewReader(body+" "))
	r.ContentLength = -1
	if got := recordResponse(t, h, r, http.StatusRequestEntityTooLarge); got.Body.String() != capped {
		t.Fatalf("streamed body limit: %q", got.Body)
	}
}

func TestSoundAndAudioListing(t *testing.T) {
	s := testState(t)
	h := s.routes()
	dir := sound.SoundDir(s.mc.DataDir)
	putFile(t, filepath.Join(dir, "b.mp3"), []byte("x"))
	putFile(t, filepath.Join(dir, "a.wav"), []byte("xy"))
	putFile(t, filepath.Join(dir, "other.ogg"), []byte("xyz"))
	w := request(t, h, http.MethodGet, "/api/sound", "", http.StatusOK)
	out := decodeResponse[struct {
		Sounds []sound.SoundFile
		Dir    string
	}](t, w.Body.Bytes())
	want := []sound.SoundFile{{File: "a.wav", Name: "a", Size: 2}, {File: "b.mp3", Name: "b", Size: 1}}
	if out.Dir != dir || !reflect.DeepEqual(out.Sounds, want) {
		t.Fatalf("sound list: %+v", out)
	}

	names, defaultName := deviceNames([]audio.DeviceInfo{{Name: "mic"}, {Name: " mic "}, {Name: "mic", IsDefault: true}, {Name: " "}, {}})
	if !reflect.DeepEqual(names, []string{"mic", " mic "}) || valueOr(defaultName, "") != "mic" {
		t.Fatalf("audio device list %v, default %v", names, defaultName)
	}
	w = request(t, h, http.MethodGet, "/api/audio/devices", "", http.StatusOK)
	deviceList := decodeResponse[map[string]json.RawMessage](t, w.Body.Bytes())
	for _, key := range []string{"input", "output"} {
		if raw := deviceList[key]; len(raw) == 0 || raw[0] != '[' {
			t.Errorf("%s devices were not an array: %s", key, raw)
		}
	}
	for _, key := range []string{"default_input", "default_output"} {
		if raw := deviceList[key]; len(raw) == 0 || (raw[0] != '"' && string(raw) != "null") {
			t.Errorf("%s was not a string or null: %s", key, raw)
		}
	}
}
