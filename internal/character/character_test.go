package character

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"testing"

	"github.com/tik-choco/tc-npc/internal/sprite"
)

func TestImportsMinimalExport(t *testing.T) {
	input := `{
		"app": "tc-town", "version": 1, "kind": "character",
		"characters": [{
			"id": "abc123", "createdAt": "2026-01-01T00:00:00Z", "updatedAt": "2026-01-02T00:00:00Z",
			"sheet": {"name": "テスト"}, "avatar": {"kind": "image", "blobKey": "avatar-img-1"}
		}]
	}`
	c := importOne(t, input)
	if c.ID != "abc123" || c.Sheet.Name != "テスト" || c.Sheet.Summary != "" || c.Avatar != nil ||
		c.CreatedAt != "2026-01-01T00:00:00Z" || c.UpdatedAt != "2026-01-02T00:00:00Z" {
		t.Errorf("unexpected character %+v", c)
	}
}

func TestImportsImageAvatarAsGeneratedSpriteSheet(t *testing.T) {
	portrait := image.NewNRGBA(image.Rect(0, 0, 4, 2))
	portrait.SetNRGBA(0, 0, color.NRGBA{R: 12, G: 34, B: 56, A: 255})
	var encoded bytes.Buffer
	if err := png.Encode(&encoded, portrait); err != nil {
		t.Fatal(err)
	}

	input := fmt.Sprintf(`{"app":"tc-town","version":1,"kind":"character","characters":[
		{"id":"withpic","sheet":{"name":"絵"},
		 "avatar":{"kind":"image","mime":"image/png","dataUrl":"data:image/png;base64,%s"}}]}`,
		base64.StdEncoding.EncodeToString(encoded.Bytes()))
	dir := t.TempDir()
	chars, err := ImportTCTownExportInto(input, dir)
	if err != nil || len(chars) != 1 {
		t.Fatalf("imported %+v, %v", chars, err)
	}
	if avatar := chars[0].Avatar; avatar == nil || *avatar != (Avatar{Kind: "sprite", File: "withpic.png"}) {
		t.Fatalf("unexpected avatar %+v", avatar)
	}
	if exists, err := sprite.SpriteExists(dir, "withpic.png"); !exists || err != nil {
		t.Errorf("generated sheet exists = %v, %v", exists, err)
	}
}

func TestBrokenImageAvatarStillImportsCharacter(t *testing.T) {
	input := `{"app":"tc-town","version":1,"kind":"character","characters":[
		{"id":"broken","sheet":{"name":"壊れ"},
		 "avatar":{"kind":"image","dataUrl":"data:image/png;base64,bm90LWFuLWltYWdl"}}]}`
	chars, err := ImportTCTownExportInto(input, t.TempDir())
	if err != nil || len(chars) != 1 || chars[0].Sheet.Name != "壊れ" || chars[0].Avatar != nil {
		t.Errorf("imported %+v, %v", chars, err)
	}
}

func TestImportsVRMAvatarAsLocalModelReference(t *testing.T) {
	input := `{"characters":[{"id":"abc123","sheet":{"name":"テスト"},
		"avatar":{"kind":"vrm","blobKey":"file-xyz","checksum":"deadbeef","fileName":"alice.vrm"}}]}`
	if avatar := importOne(t, input).Avatar; avatar == nil || *avatar != (Avatar{Kind: "vrm", File: "alice.vrm"}) {
		t.Errorf("unexpected avatar %+v", avatar)
	}
}

func TestDropsVRMAvatarWithUnsafeFileName(t *testing.T) {
	input := `{"characters":[{"id":"abc123","sheet":{"name":"テスト"},
		"avatar":{"kind":"vrm","fileName":"../../secrets.vrm"}}]}`
	if avatar := importOne(t, input).Avatar; avatar != nil {
		t.Errorf("unexpected avatar %+v", avatar)
	}
}

func TestImportsCharacterWithoutAvatarField(t *testing.T) {
	input := `{"characters":[{"id":"abc","sheet":{"name":"テスト"}}]}`
	if avatar := importOne(t, input).Avatar; avatar != nil {
		t.Errorf("unexpected avatar %+v", avatar)
	}
}

func importOne(t *testing.T, input string) Character {
	t.Helper()
	chars, err := ImportTCTownExport(input)
	if err != nil || len(chars) != 1 {
		t.Fatalf("imported %+v, %v", chars, err)
	}
	return chars[0]
}

func TestCharacterIDsThatWouldEscapeFolderAreRejected(t *testing.T) {
	for _, id := range []string{"../evil", "a/b", `a\b`, "C:evil", "", ".hidden", "a..b"} {
		if err := validateCharacterID(id); err == nil {
			t.Errorf("accepted %q", id)
		}
	}
	for _, id := range []string{"0f8fad5b-d9cb-469f-a165-70867728950e", "みどり_01"} {
		if err := validateCharacterID(id); err != nil {
			t.Errorf("rejected %q: %v", id, err)
		}
	}
}

func TestSaveAndLoadRefuseEscapingID(t *testing.T) {
	dir := t.TempDir()
	const id = "../escaped"
	if err := SaveCharacter(dir, &Character{ID: id}); err == nil {
		t.Error("save accepted invalid id")
	}
	if _, err := LoadCharacter(dir, id); err == nil {
		t.Error("load accepted invalid id")
	}
}

func TestPersonaPromptSkipsEmptySections(t *testing.T) {
	sheet := CharacterSheet{Name: "テスト", Summary: "　概要テキスト ", Persona: "　\n"}
	want := "あなたは「テスト」というキャラクターです。以下の設定に従って、そのキャラクター本人として応答してください。\n\n# 概要\n概要テキスト\n"
	if got := PersonaPrompt(&sheet); got != want {
		t.Errorf("prompt = %q; want %q", got, want)
	}
}

func TestCharacterStoreRustShapeAndActiveCharacter(t *testing.T) {
	dir := t.TempDir()
	if err := os.MkdirAll(charactersDir(dir), 0o755); err != nil {
		t.Fatal(err)
	}
	path := filepath.Join(charactersDir(dir), "abc.json")
	const legacy = `{"id":"abc","sheet":{"name":"テスト","speech_style":"丁寧"}}`
	if err := os.WriteFile(path, []byte(legacy), 0o644); err != nil {
		t.Fatal(err)
	}
	c, err := ActiveCharacter(dir, "abc")
	if err != nil || c == nil || c.Sheet.SpeechStyle != "丁寧" || c.Avatar != nil {
		t.Fatalf("active character %+v, %v", c, err)
	}
	c.UpdatedAt = "2026-01-02T00:00:00Z"
	if err := SaveCharacter(dir, c); err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var compact bytes.Buffer
	if err := json.Compact(&compact, data); err != nil {
		t.Fatal(err)
	}
	const want = `{"id":"abc","created_at":"","updated_at":"2026-01-02T00:00:00Z","sheet":{"name":"テスト","summary":"","persona":"","speech_style":"丁寧","likes":"","relationships":"","notes":""},"voice_model":null,"voice_name":null,"avatar":null}`
	if compact.String() != want {
		t.Errorf("saved %s; want %s", compact.String(), want)
	}
	for _, id := range []string{"", "absent", "../invalid"} {
		if active, err := ActiveCharacter(dir, id); active != nil || err != nil {
			t.Errorf("active %q = %+v, %v", id, active, err)
		}
	}
}
