package sprite

import (
	"bytes"
	"encoding/base64"
	"image"
	"image/color"
	"image/png"
	"testing"
)

func sampleSheet(t *testing.T) image.Image {
	t.Helper()
	portrait := image.NewNRGBA(image.Rect(0, 0, 2, 1))
	portrait.SetNRGBA(0, 0, color.NRGBA{R: 255, A: 255})
	portrait.SetNRGBA(1, 0, color.NRGBA{B: 255, A: 255})
	var encoded bytes.Buffer
	if err := png.Encode(&encoded, portrait); err != nil {
		t.Fatal(err)
	}
	dataURL := "data:image/png;base64," + base64.StdEncoding.EncodeToString(encoded.Bytes())
	sheet, err := SheetFromImageDataURL(dataURL)
	if err != nil {
		t.Fatal(err)
	}
	decoded, err := png.Decode(bytes.NewReader(sheet))
	if err != nil {
		t.Fatal(err)
	}
	return decoded
}

func TestGeneratedSheetIsOneRowOfConventionalWidth(t *testing.T) {
	want := image.Pt(generatedCellPx*generatedColumns, generatedCellPx)
	if got := sampleSheet(t).Bounds().Size(); got != want {
		t.Errorf("sheet size %v; want %v", got, want)
	}
}

func TestGeneratedCellsAreAllIdentical(t *testing.T) {
	sheet := sampleSheet(t)
	for column := 1; column < generatedColumns; column++ {
		for y := 0; y < generatedCellPx; y += 37 {
			for x := 0; x < generatedCellPx; x += 37 {
				if sheet.At(x, y) != sheet.At(column*generatedCellPx+x, y) {
					t.Errorf("cell %d differs from cell 0 at (%d,%d)", column, x, y)
				}
			}
		}
	}
}

func TestGeneratorRejectsInvalidImageDataURLs(t *testing.T) {
	for _, data := range []string{
		"https://example.invalid/a.png",
		"data:image/png;base64,!!!not-base64",
		"data:image/png;base64,aGVsbG8=",
	} {
		if _, err := SheetFromImageDataURL(data); err == nil {
			t.Errorf("accepted %q", data)
		}
	}
}
