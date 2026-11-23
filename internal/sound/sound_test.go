package sound

import (
	"os"
	"path/filepath"
	"testing"
)

func TestValidateRejectsTraversalAndSeparators(t *testing.T) {
	for _, file := range []string{"../evil.wav", "a/b.wav", `a\b.wav`, "C:chime.wav", "", ".hidden.wav", "a..b.wav"} {
		if err := ValidateSoundFileName(file); err == nil {
			t.Errorf("accepted %q", file)
		}
	}
}

func TestValidateAcceptsDecodableExtensionsCaseInsensitively(t *testing.T) {
	for _, file := range []string{"chime.wav", "Chime.WAV", "bgm.mp3", "チャイム.wav"} {
		if err := ValidateSoundFileName(file); err != nil {
			t.Errorf("rejected %q: %v", file, err)
		}
	}
	for _, file := range []string{"bgm.flac", "chime"} {
		if err := ValidateSoundFileName(file); err == nil {
			t.Errorf("accepted %q", file)
		}
	}
}

func TestListIsEmptyWhenFolderIsAbsent(t *testing.T) {
	if listed, err := ListSounds(t.TempDir()); len(listed) != 0 || err != nil {
		t.Fatalf("got %v, %v for missing library", listed, err)
	}
}

func TestListReportsDecodableFilesSortedAndIgnoresOthers(t *testing.T) {
	dir := t.TempDir()
	for _, file := range []string{"zzz.wav", "aaa.MP3", "notes.txt", "track.flac"} {
		writeSound(t, dir, file)
	}
	listed, err := ListSounds(dir)
	if err != nil || len(listed) != 2 {
		t.Fatalf("listed %+v, %v", listed, err)
	}
	if listed[0] != (SoundFile{File: "aaa.MP3", Name: "aaa", Size: 10}) || listed[1].Name != "zzz" {
		t.Errorf("unexpected listing %+v", listed)
	}
}

func TestResolveSoundPath(t *testing.T) {
	dir := t.TempDir()
	writeSound(t, dir, "chime.wav")
	absolute := filepath.Join(t.TempDir(), "chime.wav")
	for _, tt := range []struct {
		name      string
		reference string
		want      string
	}{
		{"bare library name", "chime.wav", filepath.Join(SoundDir(dir), "chime.wav")},
		{"data directory relative", "sound/chime.wav", filepath.Join(dir, "sound", "chime.wav")},
		{"missing unchanged", "absent.wav", "absent.wav"},
		{"empty unchanged", "", ""},
		{"absolute unchanged", absolute, absolute},
	} {
		t.Run(tt.name, func(t *testing.T) {
			if got := ResolveSoundPath(dir, tt.reference); got != tt.want {
				t.Errorf("resolved %q to %q; want %q", tt.reference, got, tt.want)
			}
		})
	}
}

func writeSound(t *testing.T, dir, file string) {
	t.Helper()
	if err := os.MkdirAll(SoundDir(dir), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(SoundDir(dir), file), []byte("fake-audio"), 0o644); err != nil {
		t.Fatal(err)
	}
}
