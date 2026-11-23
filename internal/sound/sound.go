// Package sound manages the chime and BGM file library.
package sound

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

// SoundFile describes an audio file without loading its bytes.
type SoundFile struct {
	File string `json:"file"`
	Name string `json:"name"`
	Size uint64 `json:"size"`
}

// SoundDir returns the sound library folder.
func SoundDir(dataDir string) string {
	return filepath.Join(dataDir, "sound")
}

// ValidateSoundFileName checks both separators and drive prefixes because
// names come from HTTP paths and must stay inside the library on every OS.
func ValidateSoundFileName(file string) error {
	if file == "" || strings.ContainsAny(file, `/\:`) || strings.Contains(file, "..") || strings.HasPrefix(file, ".") {
		return fmt.Errorf("invalid sound file name: %q", file)
	}
	if !hasSoundExtension(file) {
		return fmt.Errorf("not a sound file: %q", file)
	}
	return nil
}

func hasSoundExtension(file string) bool {
	// WAV and MP3 are the formats the playback path can decode.
	ext := filepath.Ext(file)
	return ext != file && (strings.EqualFold(ext, ".wav") || strings.EqualFold(ext, ".mp3"))
}

// ListSounds lists the playable library entries by display name.
func ListSounds(dataDir string) ([]SoundFile, error) {
	dir := SoundDir(dataDir)
	out := []SoundFile{}
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return out, nil
	}
	if err != nil {
		return nil, err
	}

	for _, entry := range entries {
		file := entry.Name()
		if !hasSoundExtension(file) {
			continue
		}
		path := filepath.Join(dir, file)
		info, err := os.Stat(path)
		if err != nil || !info.Mode().IsRegular() {
			continue
		}
		if !utf8.ValidString(file) {
			slog.Warn("skipping sound with non-UTF-8 file name", "path", path)
			continue
		}
		var size uint64
		if info, err := entry.Info(); err == nil {
			size = uint64(info.Size())
		}
		out = append(out, SoundFile{File: file, Name: strings.TrimSuffix(file, filepath.Ext(file)), Size: size})
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out, nil
}

// ResolveSoundPath resolves an operator-configured sound reference. Relative
// references try the library and then the data directory. The fallback is
// always the original reference, preserving working-directory-relative paths
// and the exact configured spelling in playback errors.
func ResolveSoundPath(dataDir, reference string) string {
	if reference == "" || filepath.IsAbs(reference) {
		return reference
	}
	for _, candidate := range []string{
		joinReference(SoundDir(dataDir), reference),
		joinReference(dataDir, reference),
	} {
		if info, err := os.Stat(candidate); err == nil && info.Mode().IsRegular() {
			return candidate
		}
	}
	return reference
}

func joinReference(base, reference string) string {
	reference = filepath.FromSlash(reference)
	// A Windows drive-relative reference supplies its own drive. A rooted
	// reference without a drive inherits the data directory's drive.
	if filepath.VolumeName(reference) != "" {
		return reference
	}
	if os.IsPathSeparator(reference[0]) {
		return filepath.VolumeName(base) + reference
	}
	// Preserve dot components until the filesystem resolves the path. A
	// symlink followed by ".." can resolve differently from filepath.Clean.
	if base == "" || os.IsPathSeparator(base[len(base)-1]) {
		return base + reference
	}
	return base + string(filepath.Separator) + reference
}
