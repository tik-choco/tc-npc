package desktop

import (
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
)

// Position is the outer window origin and size is the client size, in
// physical pixels, as in tauri-plugin-window-state.
type windowState struct {
	X      int32 `json:"x"`
	Y      int32 `json:"y"`
	Width  int32 `json:"width"`
	Height int32 `json:"height"`
}

func (s windowState) valid() bool {
	return s.Width > 0 && s.Height > 0
}

func readWindowState(path string) (windowState, error) {
	var state windowState
	data, err := os.ReadFile(path)
	if err != nil {
		return state, err
	}
	if err := json.Unmarshal(data, &state); err != nil {
		return windowState{}, fmt.Errorf("decode window state: %w", err)
	}
	if !state.valid() {
		return windowState{}, fmt.Errorf("invalid window dimensions")
	}
	return state, nil
}

func readTauriWindowState(path, label string) (windowState, error) {
	var states map[string]struct {
		windowState
		Maximized bool  `json:"maximized"`
		PreviousX int32 `json:"prev_x"`
		PreviousY int32 `json:"prev_y"`
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return windowState{}, err
	}
	if err := json.Unmarshal(data, &states); err != nil {
		return windowState{}, fmt.Errorf("decode Tauri window state: %w", err)
	}
	legacy, ok := states[label]
	if !ok {
		return windowState{}, os.ErrNotExist
	}
	state := legacy.windowState
	if legacy.Maximized {
		state.X, state.Y = legacy.PreviousX, legacy.PreviousY
	}
	if !state.valid() {
		return windowState{}, fmt.Errorf("invalid Tauri window dimensions")
	}
	return state, nil
}

func writeWindowState(path string, state windowState) error {
	if !state.valid() {
		return fmt.Errorf("invalid window dimensions")
	}
	data, err := json.MarshalIndent(state, "", "  ")
	if err != nil {
		return fmt.Errorf("encode window state: %w", err)
	}
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return err
	}
	temp, err := os.CreateTemp(dir, ".window-state-*.tmp")
	if err != nil {
		return err
	}
	defer os.Remove(temp.Name())
	_, writeErr := temp.Write(data)
	closeErr := temp.Close()
	if writeErr != nil {
		return writeErr
	}
	if closeErr != nil {
		return closeErr
	}
	return os.Rename(temp.Name(), path)
}
