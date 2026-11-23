package tui

import (
	"slices"
	"strconv"
	"strings"
	"testing"
)

func TestDisplayWidth(t *testing.T) {
	for text, want := range map[string]int{"hello": 5, "ステータス": 10, "v1 キャラ": 9} {
		if got := displayWidth(text); got != want {
			t.Errorf("displayWidth(%q) = %d, want %d", text, got, want)
		}
	}
}

func TestTruncateDisplay(t *testing.T) {
	for _, tc := range []struct {
		text  string
		width int
		want  string
	}{
		{"hi", 10, "hi"},
		{"ステータス", 10, "ステータス"},
		{"abcdefghij", 6, "abc..."},
		{"abcdef", 3, "abc"},
		{"abcdefgh", 5, "ab..."},
		{"あいうえおかきく", 6, "あ..."},
		{"あb", 1, "b"},
	} {
		if got := truncateDisplay(tc.text, tc.width); got != tc.want {
			t.Errorf("truncateDisplay(%q, %d) = %q, want %q", tc.text, tc.width, got, tc.want)
		}
	}
}

func TestTruncateDisplayNeverExceedsWidth(t *testing.T) {
	for _, text := range []string{"hello world, this is long", "日本語のテキストをたくさん入れる"} {
		for width := range 40 {
			got := truncateDisplay(text, width)
			if displayWidth(got) > width {
				t.Errorf("width %d: %q exceeds width with %d cells", width, got, displayWidth(got))
			}
		}
	}
}

func TestPadDisplayWidth(t *testing.T) {
	for _, text := range []string{"talk", "キャラ"} {
		if got := padDisplayWidth(text, 10); displayWidth(got) != 10 {
			t.Errorf("%q did not pad to ten columns: %q", text, got)
		}
	}
	if padDisplayWidth("ステータス", 4) != "ステータス" || padDisplayWidth("hello", 5) != "hello" {
		t.Fatal("padding changed content already at or above the bound")
	}
}

func TestWrapLinesFitsASCIIJapaneseAndMixedText(t *testing.T) {
	for _, tc := range []struct {
		text   string
		widths []int
	}{
		{"the quick brown fox jumps over the lazy dog again and again", []int{1, 3, 8, 20}},
		{"これはとても長い日本語の文章でスペースが一切含まれていません", []int{1, 2, 5, 12}},
		{"status: 接続中です メモリ使用量が高くなっています warning", []int{4, 7, 15}},
	} {
		for _, width := range tc.widths {
			for _, row := range wrapLines(tc.text, width) {
				if displayWidth(row) > width {
					t.Errorf("row %q exceeds width %d", row, width)
				}
			}
		}
	}
}

func TestWrapLinesBlankInputYieldsOneBlankLine(t *testing.T) {
	for _, text := range []string{"", "   "} {
		if got := wrapLines(text, 10); !slices.Equal(got, []string{""}) {
			t.Errorf("wrapLines(%q) = %q, want one blank row", text, got)
		}
	}
}

func TestSplitLongLinesFits(t *testing.T) {
	lines := []string{strings.Repeat("a", 50), strings.Repeat("日本語", 20)}
	for _, width := range []int{0, 1, 2, 3, 9} {
		for _, row := range splitLongLines(lines, width) {
			if displayWidth(row) > width {
				t.Errorf("row %q exceeds %d columns", row, width)
			}
		}
	}
}

func TestCropLines(t *testing.T) {
	lines := make([]string, 10)
	for i := range lines {
		lines[i] = strconv.Itoa(i)
	}
	if got := cropLines(lines, 3); !slices.Equal(got, []string{"7", "8", "9"}) {
		t.Fatalf("cropLines = %q, want 7, 8, 9", got)
	}
	for height := range 8 {
		if got := cropLines(lines[:5], height); len(got) > height {
			t.Errorf("cropped length %d exceeds height %d", len(got), height)
		}
	}
	if got := cropLines(lines, 0); len(got) != 0 {
		t.Fatalf("zero height yielded %q", got)
	}
}

func TestFitLines(t *testing.T) {
	lines := []string{"日本語の長い一行テキストです", "short"}
	for height := range 6 {
		for _, width := range []int{0, 1, 3, 5, 7, 15, 20} {
			got := fitLines(lines, width, height)
			if len(got) != height {
				t.Errorf("width %d: got %d rows, want %d", width, len(got), height)
			}
			for _, row := range got {
				if displayWidth(row) > width {
					t.Errorf("fitLines row %q exceeds width %d", row, width)
				}
			}
		}
	}
	if got := fitLines([]string{"one"}, 10, 4); !slices.Equal(got, []string{"one", "", "", ""}) {
		t.Errorf("blank row padding yielded %q", got)
	}
}

func TestWidgetText(t *testing.T) {
	if got := chip("", colorPrimary); got.text != "  " {
		t.Errorf("empty chip = %q", got.text)
	}
	for _, active := range []bool{false, true} {
		if got := node("状態", active); got.text != " 状態 " {
			t.Errorf("node = %q", got.text)
		}
	}
	if got := edge("送信").text; got != "--送信-->" {
		t.Errorf("edge = %q", got)
	}
}
