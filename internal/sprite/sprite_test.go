package sprite

import (
	"github.com/tik-choco/tc-npc/internal/vrm"
	"os"
	"path/filepath"
	"testing"
)

func TestValidateRejectsTraversalAndSeparators(t *testing.T) {
	for _, file := range []string{"../evil.png", "a/b.png", `a\b.png`, "C:sheet.png", "", ".hidden.png", "a..b.png"} {
		if err := ValidateSpriteFileName(file); err == nil {
			t.Errorf("accepted %q", file)
		}
	}
}

func TestSpriteExistsDistinguishesInvalidFromMissing(t *testing.T) {
	dir := t.TempDir()
	if _, err := SpriteExists(dir, "../escape.png"); err == nil {
		t.Error("expected invalid name error")
	}
	if exists, err := SpriteExists(dir, "absent.png"); exists || err != nil {
		t.Errorf("got %v, %v for missing file", exists, err)
	}
}

func TestValidateRejectsWrongExtension(t *testing.T) {
	for _, file := range []string{"model.vrm", "sheet"} {
		if err := ValidateSpriteFileName(file); err == nil {
			t.Errorf("accepted %q", file)
		}
	}
}

func TestValidateAcceptsPlainNamesCaseInsensitively(t *testing.T) {
	for _, file := range []string{"alice.png", "Alice.PNG", "日本語のシート.png"} {
		if err := ValidateSpriteFileName(file); err != nil {
			t.Errorf("rejected %q: %v", file, err)
		}
	}
}

func TestListIsEmptyWhenFolderIsAbsent(t *testing.T) {
	if listed, err := ListSprites(t.TempDir()); len(listed) != 0 || err != nil {
		t.Fatalf("got %v, %v for missing library", listed, err)
	}
}

func TestSaveListReadDeleteRoundTrip(t *testing.T) {
	dir := t.TempDir()
	const file = "alice.png"
	const content = "fake-png"
	saved, err := SaveSprite(dir, file, []byte(content))
	if err != nil || saved.File != file || saved.Name != "alice" || saved.Size != 8 {
		t.Fatalf("saved %+v, %v", saved, err)
	}
	listed, err := ListSprites(dir)
	if err != nil || len(listed) != 1 || listed[0] != saved {
		t.Fatalf("listed %+v, %v", listed, err)
	}
	if data, err := ReadSprite(dir, file); string(data) != content || err != nil {
		t.Errorf("got %q, %v", data, err)
	}
	if exists, err := SpriteExists(dir, file); !exists || err != nil {
		t.Errorf("exists = %v, %v", exists, err)
	}
	if removed, err := DeleteSprite(dir, file); !removed || err != nil {
		t.Errorf("removed = %v, %v", removed, err)
	}
	if exists, err := SpriteExists(dir, file); exists || err != nil {
		t.Errorf("exists after delete = %v, %v", exists, err)
	}
	if data, err := ReadSprite(dir, file); data != nil || err != nil {
		t.Errorf("read after delete = %q, %v", data, err)
	}
	if removed, err := DeleteSprite(dir, file); removed || err != nil {
		t.Errorf("second delete = %v, %v", removed, err)
	}
}

func TestListIgnoresOtherFiles(t *testing.T) {
	dir := t.TempDir()
	libraryDir := SpriteDir(dir)
	if err := os.MkdirAll(libraryDir, 0o755); err != nil {
		t.Fatal(err)
	}
	for _, file := range []string{"notes.txt", "model.vrm"} {
		if err := os.WriteFile(filepath.Join(libraryDir, file), []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := SaveSprite(dir, "bob.PNG", []byte("x")); err != nil {
		t.Fatal(err)
	}
	if listed, err := ListSprites(dir); err != nil || len(listed) != 1 || listed[0].File != "bob.PNG" {
		t.Fatalf("listed %+v, %v", listed, err)
	}
}

func TestSpriteDirIsSeparateFromVRMDir(t *testing.T) {
	dir := t.TempDir()
	if SpriteDir(dir) == vrm.VRMDir(dir) {
		t.Fatal("sprite and VRM library directories must differ")
	}
}
