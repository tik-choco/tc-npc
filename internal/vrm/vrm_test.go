package vrm

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestValidateRejectsTraversalAndSeparators(t *testing.T) {
	for _, file := range []string{"../evil.vrm", "a/b.vrm", `a\b.vrm`, "C:model.vrm", "", ".hidden.vrm", "a..b.vrm"} {
		if err := ValidateVRMFileName(file); err == nil {
			t.Errorf("accepted %q", file)
		}
	}
}

func TestVRMExistsDistinguishesInvalidFromMissing(t *testing.T) {
	dir := t.TempDir()
	if _, err := VRMExists(dir, "../escape.vrm"); err == nil {
		t.Error("expected invalid name error")
	}
	if exists, err := VRMExists(dir, "absent.vrm"); exists || err != nil {
		t.Errorf("got %v, %v for missing file", exists, err)
	}
}

func TestValidateRejectsWrongExtension(t *testing.T) {
	for _, file := range []string{"model.png", "model"} {
		if err := ValidateVRMFileName(file); err == nil {
			t.Errorf("accepted %q", file)
		}
	}
}

func TestValidateAcceptsPlainNamesCaseInsensitively(t *testing.T) {
	for _, file := range []string{"alice.vrm", "Alice.VRM", "日本語のモデル.vrm"} {
		if err := ValidateVRMFileName(file); err != nil {
			t.Errorf("rejected %q: %v", file, err)
		}
	}
}

func TestListIsEmptyWhenFolderIsAbsent(t *testing.T) {
	if listed, err := ListVRMModels(t.TempDir()); len(listed) != 0 || err != nil {
		t.Fatalf("got %v, %v for missing library", listed, err)
	}
}

func TestSaveListReadDeleteRoundTrip(t *testing.T) {
	dir := t.TempDir()
	const file = "alice.vrm"
	const content = "fake-bytes"
	saved, err := SaveVRM(dir, file, []byte(content))
	if err != nil || saved.File != file || saved.Name != "alice" || saved.Size != 10 {
		t.Fatalf("saved %+v, %v", saved, err)
	}
	listed, err := ListVRMModels(dir)
	if err != nil || len(listed) != 1 || listed[0] != saved {
		t.Fatalf("listed %+v, %v", listed, err)
	}
	if data, err := ReadVRM(dir, file); string(data) != content || err != nil {
		t.Errorf("got %q, %v", data, err)
	}
	if exists, err := VRMExists(dir, file); !exists || err != nil {
		t.Errorf("exists = %v, %v", exists, err)
	}
	if removed, err := DeleteVRM(dir, file); !removed || err != nil {
		t.Errorf("removed = %v, %v", removed, err)
	}
	if exists, err := VRMExists(dir, file); exists || err != nil {
		t.Errorf("exists after delete = %v, %v", exists, err)
	}
	if data, err := ReadVRM(dir, file); data != nil || err != nil {
		t.Errorf("read after delete = %q, %v", data, err)
	}
	if removed, err := DeleteVRM(dir, file); removed || err != nil {
		t.Errorf("second delete = %v, %v", removed, err)
	}
}

func TestListIgnoresOtherFiles(t *testing.T) {
	dir := t.TempDir()
	libraryDir := VRMDir(dir)
	if err := os.MkdirAll(libraryDir, 0o755); err != nil {
		t.Fatal(err)
	}
	for _, file := range []string{"notes.txt", "model.png"} {
		if err := os.WriteFile(filepath.Join(libraryDir, file), []byte("x"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	if _, err := SaveVRM(dir, "bob.VRM", []byte("x")); err != nil {
		t.Fatal(err)
	}
	if listed, err := ListVRMModels(dir); err != nil || len(listed) != 1 || listed[0].File != "bob.VRM" {
		t.Fatalf("listed %+v, %v", listed, err)
	}
}

func TestVRMFileStatRejectsInvalidName(t *testing.T) {
	if _, err := VRMFileStat(t.TempDir(), "../escape.vrm"); err == nil {
		t.Fatal("expected invalid name error")
	}
}

func TestVRMFileStatReturnsNilForMissingModel(t *testing.T) {
	if stat, err := VRMFileStat(t.TempDir(), "absent.vrm"); stat != nil || err != nil {
		t.Fatalf("got %+v, %v for missing file", stat, err)
	}
}

func TestVRMFileStatReportsSizeAndModificationTime(t *testing.T) {
	dir := t.TempDir()
	before := time.Now()
	if _, err := SaveVRM(dir, "alice.vrm", []byte("fake-bytes")); err != nil {
		t.Fatal(err)
	}
	after := time.Now()
	stat, err := VRMFileStat(dir, "alice.vrm")
	if err != nil || stat == nil {
		t.Fatalf("got %+v, %v", stat, err)
	}
	const tolerance = 2 * time.Second
	if stat.Size != 10 || stat.Modified.Before(before.Add(-tolerance)) || stat.Modified.After(after.Add(tolerance)) {
		t.Errorf("unexpected stat %+v", stat)
	}
}
