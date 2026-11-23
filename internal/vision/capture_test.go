package vision

import (
	"bytes"
	"encoding/base64"
	"image"
	"image/color"
	"image/png"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestDownscale(t *testing.T) {
	for _, tc := range []struct {
		width, height, wantWidth, wantHeight int
	}{
		{1280, 2, 1280, 2},
		{1920, 1080, 1280, 720},
		{1080, 1920, 720, 1280},
		{3000, 999, 1279, 426},
		{1, 4000, 1, 1280},
	} {
		img := image.NewRGBA(image.Rect(0, 0, tc.width, tc.height))
		got := downscale(img)
		want := image.Pt(tc.wantWidth, tc.wantHeight)
		if got.Bounds().Size() != want {
			t.Errorf("downscale %v = %v, want %v", img.Bounds().Size(), got.Bounds().Size(), want)
		}
		if max(tc.width, tc.height) <= maxEdge && got != img {
			t.Errorf("downscale changed image within limit")
		}
	}
}

func TestCapturePNG(t *testing.T) {
	img := image.NewRGBA(image.Rect(0, 0, 2, 1))
	pixel := color.RGBA{R: 16, G: 32, B: 64, A: 255}
	img.SetRGBA(0, 0, pixel)
	img.SetRGBA(1, 0, pixel)
	url, err := encodeDataURL(img)
	if err != nil {
		t.Fatal(err)
	}
	raw, ok := strings.CutPrefix(url, "data:image/png;base64,")
	if !ok {
		t.Fatal("expected a PNG data URL")
	}
	data, err := base64.StdEncoding.DecodeString(raw)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := png.Decode(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	if decoded.Bounds() != img.Bounds() || color.RGBAModel.Convert(decoded.At(0, 0)) != pixel {
		t.Errorf("unexpected PNG bounds or color: %v, %v", decoded.Bounds(), decoded.At(0, 0))
	}
	if data[25] != 6 {
		t.Errorf("PNG color type = %d, want RGBA (6)", data[25])
	}

	path := filepath.Join(t.TempDir(), "data", "debug_capture.png")
	if err := saveDebugPNG(path, img); err != nil {
		t.Fatal(err)
	}
	if err := saveDebugPNG(path, img); err != nil {
		t.Fatal(err)
	}
	file, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Equal(file, data) {
		t.Error("debug PNG differs from the PNG sent to the model")
	}
}

func TestWindowMatcher(t *testing.T) {
	for _, tc := range []struct {
		target, value string
		app, match    bool
	}{
		{"VRChat.EXE", "VRChat.exe", true, true},
		{"chrome.exe", "Google Chrome", true, true},
		{"VRChat.exe", "Other App", true, false},
		{"VRCHAT", "a VRChat window", false, true},
		{"VRCHAT", "Other Window", false, false},
	} {
		app, matches := windowMatcher(tc.target)
		if app != tc.app || matches(tc.value) != tc.match {
			t.Errorf("unexpected match for target %q and value %q", tc.target, tc.value)
		}
	}
}
