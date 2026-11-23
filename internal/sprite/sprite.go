// Package sprite manages 2D sprite sheets in the local sprites library.
package sprite

import (
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"unicode/utf8"
)

// SpriteSheet describes a library entry without loading its bytes.
type SpriteSheet struct {
	File string `json:"file"`
	Name string `json:"name"`
	Size uint64 `json:"size"`
}

// SpriteDir returns the library folder.
func SpriteDir(dataDir string) string {
	return filepath.Join(dataDir, "sprites")
}

// ValidateSpriteFileName checks both separators and drive prefixes because
// names come from HTTP paths and must stay inside the library on every OS.
func ValidateSpriteFileName(file string) error {
	if file == "" || strings.ContainsAny(file, `/\:`) || strings.Contains(file, "..") || strings.HasPrefix(file, ".") {
		return fmt.Errorf("invalid sprite file name: %q", file)
	}
	if !hasExtension(file) {
		return fmt.Errorf("not a .png file: %q", file)
	}
	return nil
}

func hasExtension(file string) bool {
	ext := filepath.Ext(file)
	return ext != file && strings.EqualFold(ext, ".png")
}

// ListSprites lists the library entries by display name.
func ListSprites(dataDir string) ([]SpriteSheet, error) {
	dir := SpriteDir(dataDir)
	out := []SpriteSheet{}
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return out, nil
	}
	if err != nil {
		return nil, err
	}

	for _, entry := range entries {
		file := entry.Name()
		if !hasExtension(file) {
			continue
		}
		path := filepath.Join(dir, file)
		info, err := os.Stat(path)
		if err != nil || !info.Mode().IsRegular() {
			continue
		}
		if !utf8.ValidString(file) {
			slog.Warn("skipping sprite with non-UTF-8 file name", "path", path)
			continue
		}
		var size uint64
		if info, err := entry.Info(); err == nil {
			size = uint64(info.Size())
		}
		out = append(out, SpriteSheet{File: file, Name: strings.TrimSuffix(file, filepath.Ext(file)), Size: size})
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out, nil
}

// SpriteExists checks whether the reference resolves without reading sprite bytes.
func SpriteExists(dataDir, file string) (bool, error) {
	if err := ValidateSpriteFileName(file); err != nil {
		return false, err
	}
	info, err := os.Stat(filepath.Join(SpriteDir(dataDir), file))
	return err == nil && info.Mode().IsRegular(), nil
}

// ReadSprite returns the file's bytes, or nil if the file is missing.
func ReadSprite(dataDir, file string) ([]byte, error) {
	if err := ValidateSpriteFileName(file); err != nil {
		return nil, err
	}
	path := filepath.Join(SpriteDir(dataDir), file)
	if _, err := os.Stat(path); err != nil {
		return nil, nil
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read sprite %s: %w", file, err)
	}
	return data, nil
}

// SaveSprite creates or replaces a library entry.
func SaveSprite(dataDir, file string, data []byte) (SpriteSheet, error) {
	if err := ValidateSpriteFileName(file); err != nil {
		return SpriteSheet{}, err
	}
	dir := SpriteDir(dataDir)
	if err := os.MkdirAll(dir, 0o777); err != nil {
		return SpriteSheet{}, err
	}
	if err := os.WriteFile(filepath.Join(dir, file), data, 0o666); err != nil {
		return SpriteSheet{}, fmt.Errorf("failed to write sprite %s: %w", file, err)
	}
	return SpriteSheet{File: file, Name: strings.TrimSuffix(file, filepath.Ext(file)), Size: uint64(len(data))}, nil
}

// DeleteSprite removes a library entry and reports whether it existed.
func DeleteSprite(dataDir, file string) (bool, error) {
	if err := ValidateSpriteFileName(file); err != nil {
		return false, err
	}
	path := filepath.Join(SpriteDir(dataDir), file)
	if _, err := os.Stat(path); err != nil {
		return false, nil
	}
	info, err := os.Lstat(path)
	if err == nil {
		if info.IsDir() {
			return false, fmt.Errorf("failed to delete sprite %s: is a directory", file)
		}
		err = os.Remove(path)
	}
	if err != nil {
		return false, fmt.Errorf("failed to delete sprite %s: %w", file, err)
	}
	return true, nil
}
