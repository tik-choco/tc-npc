// Package vrm manages VRM avatar model files in the local vrm library.
package vrm

import (
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
	"unicode/utf8"
)

// VRMModel describes a library entry without loading its bytes.
type VRMModel struct {
	File string `json:"file"`
	Name string `json:"name"`
	Size uint64 `json:"size"`
}

// VRMDir returns the library folder.
func VRMDir(dataDir string) string {
	return filepath.Join(dataDir, "vrm")
}

// ValidateVRMFileName checks both separators and drive prefixes because
// names come from HTTP paths and must stay inside the library on every OS.
func ValidateVRMFileName(file string) error {
	if file == "" || strings.ContainsAny(file, `/\:`) || strings.Contains(file, "..") || strings.HasPrefix(file, ".") {
		return fmt.Errorf("invalid vrm file name: %q", file)
	}
	if !hasExtension(file) {
		return fmt.Errorf("not a .vrm file: %q", file)
	}
	return nil
}

func hasExtension(file string) bool {
	ext := filepath.Ext(file)
	return ext != file && strings.EqualFold(ext, ".vrm")
}

// ListVRMModels lists the library entries by display name.
func ListVRMModels(dataDir string) ([]VRMModel, error) {
	dir := VRMDir(dataDir)
	out := []VRMModel{}
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
			slog.Warn("skipping vrm with non-UTF-8 file name", "path", path)
			continue
		}
		var size uint64
		if info, err := entry.Info(); err == nil {
			size = uint64(info.Size())
		}
		out = append(out, VRMModel{File: file, Name: strings.TrimSuffix(file, filepath.Ext(file)), Size: size})
	}
	sort.SliceStable(out, func(i, j int) bool { return out[i].Name < out[j].Name })
	return out, nil
}

// VRMExists checks whether the reference resolves without reading model bytes.
func VRMExists(dataDir, file string) (bool, error) {
	if err := ValidateVRMFileName(file); err != nil {
		return false, err
	}
	info, err := os.Stat(filepath.Join(VRMDir(dataDir), file))
	return err == nil && info.Mode().IsRegular(), nil
}

// FileStat contains the inputs for a conditional GET's cache validator.
type FileStat struct {
	Size     uint64
	Modified time.Time
}

// VRMFileStat returns the size and modification time of a model.
// Modification time also catches same-size uploads over an existing model.
func VRMFileStat(dataDir, file string) (*FileStat, error) {
	if err := ValidateVRMFileName(file); err != nil {
		return nil, err
	}
	info, err := os.Stat(filepath.Join(VRMDir(dataDir), file))
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("failed to stat vrm %s: %w", file, err)
	}
	if !info.Mode().IsRegular() {
		return nil, nil
	}
	return &FileStat{Size: uint64(info.Size()), Modified: info.ModTime()}, nil
}

// ReadVRM returns the file's bytes, or nil if the file is missing.
func ReadVRM(dataDir, file string) ([]byte, error) {
	if err := ValidateVRMFileName(file); err != nil {
		return nil, err
	}
	path := filepath.Join(VRMDir(dataDir), file)
	if _, err := os.Stat(path); err != nil {
		return nil, nil
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return nil, fmt.Errorf("failed to read vrm %s: %w", file, err)
	}
	return data, nil
}

// SaveVRM creates or replaces a library entry.
func SaveVRM(dataDir, file string, data []byte) (VRMModel, error) {
	if err := ValidateVRMFileName(file); err != nil {
		return VRMModel{}, err
	}
	dir := VRMDir(dataDir)
	if err := os.MkdirAll(dir, 0o777); err != nil {
		return VRMModel{}, err
	}
	if err := os.WriteFile(filepath.Join(dir, file), data, 0o666); err != nil {
		return VRMModel{}, fmt.Errorf("failed to write vrm %s: %w", file, err)
	}
	return VRMModel{File: file, Name: strings.TrimSuffix(file, filepath.Ext(file)), Size: uint64(len(data))}, nil
}

// DeleteVRM removes a library entry and reports whether it existed.
func DeleteVRM(dataDir, file string) (bool, error) {
	if err := ValidateVRMFileName(file); err != nil {
		return false, err
	}
	path := filepath.Join(VRMDir(dataDir), file)
	if _, err := os.Stat(path); err != nil {
		return false, nil
	}
	info, err := os.Lstat(path)
	if err == nil {
		if info.IsDir() {
			return false, fmt.Errorf("failed to delete vrm %s: is a directory", file)
		}
		err = os.Remove(path)
	}
	if err != nil {
		return false, fmt.Errorf("failed to delete vrm %s: %w", file, err)
	}
	return true, nil
}
