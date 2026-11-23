package mist

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

func TestDLLLookupOrder(t *testing.T) {
	exeDLL := filepath.Join(t.TempDir(), mistDLLName)
	dataDir := t.TempDir()
	dataDLL := filepath.Join(dataDir, mistDLLName)
	override := filepath.Join(t.TempDir(), "custom.dll")
	exe := filepath.Join(filepath.Dir(exeDLL), "tc-npc.exe")
	if _, err := findDLL(override, exe, dataDir); err == nil || !strings.Contains(err.Error(), mistDLLEnv) || !strings.Contains(err.Error(), dataDLL) {
		t.Fatalf("missing DLL error = %v", err)
	}
	for _, expected := range []string{dataDLL, exeDLL, override} {
		if err := os.WriteFile(expected, nil, 0o600); err != nil {
			t.Fatal(err)
		}
		if path, err := findDLL(override, exe, dataDir); err != nil || path != expected {
			t.Fatalf("lookup = %q, %v, want %q", path, err, expected)
		}
	}
}

func TestDLLIntegration(t *testing.T) {
	if os.Getenv("TC_NPC_MIST_DLL_TEST") != "1" {
		t.Skip("set TC_NPC_MIST_DLL_TEST=1 to load the official mistlib DLL")
	}
	path := os.Getenv(mistDLLEnv)
	if path == "" {
		t.Fatalf("set %s to the path of mistlib.dll (mistlib release asset mistlib-native-windows-x86_64)", mistDLLEnv)
	}
	e, err := openDLL(path)
	if err != nil {
		t.Fatal(err)
	}
	version, err := readDLLString(e.getVersion)
	if err != nil {
		t.Fatal(err)
	}
	info, err := readDLLString(e.getBuildInfo)
	if err != nil {
		t.Fatal(err)
	}
	t.Logf("get_version: %s", version)
	t.Logf("get_build_info: %s", info)
	var build struct {
		Version string `json:"version"`
		Commit  string `json:"commit"`
		Target  string `json:"target"`
	}
	if err := json.Unmarshal([]byte(info), &build); err != nil || version != "0.7.3-beta.6" ||
		build.Version != version || build.Commit == "" || build.Target == "" {
		t.Fatalf("unexpected DLL build: %s, %s (%v)", version, info, err)
	}

	// Joining before initialization produces a room-scoped failure locally,
	// exercising the real callback ABI without contacting signaling servers.
	events := newEngineEvents()
	e.events.Store(events)
	defer e.events.Store(nil)
	e.registerEvent.Call(syscall.NewCallback(e.onEvent))
	e.joinRoom("ffi-test")
	select {
	case <-events.ready:
		event := events.drain()
		if event == nil || event.kind != eventRoomJoinFailed || event.room != "ffi-test" || len(event.data) == 0 {
			t.Fatalf("unexpected v2 callback: %+v", event)
		}
		t.Logf("v2 callback: %s / %s", event.room, event.data)
	case <-time.After(5 * time.Second):
		t.Fatal("mistlib did not report the room join failure")
	}
}
