package tui

import (
	"testing"

	"github.com/charmbracelet/lipgloss"
)

func TestPalette(t *testing.T) {
	got := []lipgloss.Color{colorPrimary, colorSecondary, colorSuccess, colorWarning, colorError, colorGray, colorText, colorMuted, colorHighlight}
	want := []lipgloss.Color{"#5A67D8", "#2DD4BF", "#04E384", "#FFB800", "#FF4B4B", "#3C3C3C", "#E1E1E1", "#666666", "#FFFFFF"}
	for i, color := range got {
		if color != want[i] {
			t.Errorf("palette[%d] = %s, want %s", i, color, want[i])
		}
	}
}

func TestNodeAndBorderStyle(t *testing.T) {
	if nodeStyle(true).GetBackground() != colorPrimary || nodeStyle(false).GetBackground() != colorGray {
		t.Fatal("node background does not change with activity")
	}
	if !borderStyle(true).GetBold() || borderStyle(false).GetBold() {
		t.Fatal("border should be bold only when focused")
	}
}
