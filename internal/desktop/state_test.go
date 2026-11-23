package desktop

import (
	"errors"
	"os"
	"path/filepath"
	"testing"
)

func TestWindowStateRoundTrip(t *testing.T) {
	path := filepath.Join(t.TempDir(), "data", "desktop-mascot.json")
	if _, err := readWindowState(path); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("read missing state: %v", err)
	}
	for _, want := range []windowState{
		{X: -640, Y: 22, Width: 360, Height: 480},
		{X: 0, Y: -24, Width: 700, Height: 640},
	} {
		if err := writeWindowState(path, want); err != nil {
			t.Fatal(err)
		}
		got, err := readWindowState(path)
		if err != nil || got != want {
			t.Fatalf("read window state = %+v, %v, want %+v", got, err, want)
		}
	}
	if err := writeWindowState(path, windowState{}); err == nil {
		t.Fatal("saved an invalid size")
	}
	entries, err := os.ReadDir(filepath.Dir(path))
	if err != nil || len(entries) != 1 {
		t.Fatalf("expected only the saved file, got %v, %v", entries, err)
	}
}

func TestReadInvalidWindowState(t *testing.T) {
	path := filepath.Join(t.TempDir(), "state.json")
	for _, raw := range []string{`{}`, `{"width": -1, "height": 300}`, `{"width": 400,`, `null`} {
		if err := os.WriteFile(path, []byte(raw), 0o600); err != nil {
			t.Fatal(err)
		}
		if _, err := readWindowState(path); err == nil {
			t.Errorf("accepted invalid state %q", raw)
		}
	}
}

func TestReadTauriWindowState(t *testing.T) {
	path := filepath.Join(t.TempDir(), ".window-state.json")
	raw := `{
		"main":{"x":0,"y":0,"width":1024,"height":768,"prev_x":20,"prev_y":-15,"maximized":true,"decorated":true,"visible":true,"fullscreen":false},
		"mascot":{"x":-750,"y":450,"width":360,"height":480,"prev_x":0,"prev_y":0,"maximized":false,"decorated":true,"visible":true,"fullscreen":false}
	}`
	if err := os.WriteFile(path, []byte(raw), 0o600); err != nil {
		t.Fatal(err)
	}
	for label, want := range map[string]windowState{
		"main":   {X: 20, Y: -15, Width: 1024, Height: 768},
		"mascot": {X: -750, Y: 450, Width: 360, Height: 480},
	} {
		got, err := readTauriWindowState(path, label)
		if err != nil || got != want {
			t.Errorf("state for %q = %+v, %v, want %+v", label, got, err, want)
		}
	}
	if _, err := readTauriWindowState(path, "absent"); !errors.Is(err, os.ErrNotExist) {
		t.Errorf("read missing Tauri label: %v", err)
	}
}
