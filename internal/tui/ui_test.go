package tui

import (
	"fmt"
	"regexp"
	"slices"
	"strings"
	"testing"
	"time"
)

var sgr = regexp.MustCompile(`\x1b\[[0-9;:]*m`)

func plain(text string) string { return sgr.ReplaceAllString(text, "") }

func populatedApp() app {
	a := app{
		character: &characterRef{ID: "c1", Name: "ちょこ"},
		modules:   moduleFlags{Talk: true, Speech: true, Scheduler: true},
	}
	a.setConnection(connected)
	for i := range 40 {
		role := "user"
		if i%2 != 0 {
			role = "assistant"
		}
		a.chat = append(a.chat, chatRow{role: role, text: fmt.Sprintf("これは%d番目のテスト発話です。全角文字を含みます。", i)})
		a.addLog("sense", fmt.Sprintf("感覚ログ %d", i))
		a.addEventLog(fmt.Sprintf("操作 %d", i))
	}
	return a
}

func TestRenderAllSizesAndStates(t *testing.T) {
	config, notice, longError := populatedApp(), populatedApp(), populatedApp()
	config.toggleView()
	notice.setNotice("設定を保存しました")
	longError.setError(strings.Repeat("非常に長いエラーメッセージです。", 20), time.Now())
	for _, size := range [][2]int{{1, 1}, {4, 2}, {10, 3}, {20, 5}, {40, 12}, {79, 23}, {80, 24}, {120, 40}, {200, 60}} {
		for name, state := range map[string]app{
			"empty": {}, "populated": populatedApp(), "long error": longError, "notice": notice, "config": config,
		} {
			width, height := size[0], size[1]
			t.Run(fmt.Sprintf("%dx%d/%s", width, height, name), func(t *testing.T) {
				out := plain(render(&state, width, height))
				lines := strings.Split(out, "\n")
				if len(lines) != height {
					t.Fatalf("got %d rows, want %d", len(lines), height)
				}
				for _, line := range lines {
					if displayWidth(line) != width {
						t.Errorf("row is %d columns, want %d: %q", displayWidth(line), width, line)
					}
				}
			})
		}
	}
	if got := render(&config, 0, 0); got != "" {
		t.Errorf("zero-size frame = %q", got)
	}
}

func TestTopBarAndFooterSurviveSmallTerminal(t *testing.T) {
	a := populatedApp()
	out := plain(render(&a, 40, 12))
	if !strings.Contains(out, "tc-npc") || !strings.Contains(out, "Ctrl+C") {
		t.Fatalf("top bar or footer missing:\n%s", out)
	}
}

func TestFullSizeShowsAllPanelsAndModuleFlow(t *testing.T) {
	a := populatedApp()
	out := plain(render(&a, 120, 40))
	for _, text := range []string{"チャット", "ログ", "イベント", "talk"} {
		if !strings.Contains(out, text) {
			t.Errorf("missing %q in full layout:\n%s", text, out)
		}
	}
}

func TestCompactDropsEventLogFirst(t *testing.T) {
	a := populatedApp()
	out := plain(render(&a, 60, 18))
	if !strings.Contains(out, "tc-npc") || strings.Contains(out, "イベント") {
		t.Fatalf("unexpected compact layout:\n%s", out)
	}
}

func TestConfigShowsModuleStatesAndRestartRequirement(t *testing.T) {
	a := populatedApp()
	a.toggleView()
	out := plain(render(&a, 80, 24))
	for _, text := range []string{"モジュール", "[x]", "[ ]", "要再起動", "キャラクタ", "(取得中/なし)"} {
		if !strings.Contains(out, text) {
			t.Errorf("missing %q in Config:\n%s", text, out)
		}
	}
}

func TestLongErrorDoesNotEvictTranscript(t *testing.T) {
	a := populatedApp()
	a.setError(strings.Repeat("非常に長いエラー메시지です。", 40), time.Now())
	out := plain(render(&a, 120, 40))
	if !strings.Contains(out, "エラー") || !strings.Contains(out, "チャット") {
		t.Fatalf("error or chat missing:\n%s", out)
	}
}

func TestScrollCountsWrappedRowsFromBottom(t *testing.T) {
	lines := []line{{raw("old")}, {raw("日本語")}, {raw("end")}}
	for _, tc := range []struct {
		scroll int
		want   []string
	}{
		{0, []string{"語", "end"}},
		{1, []string{"日本", "語"}},
		{99, []string{"old", "日本"}},
	} {
		rows := visibleRows(lines, 4, 2, tc.scroll)
		var got []string
		for _, row := range rows {
			got = append(got, strings.TrimSpace(renderGlyphs(row, 4)))
		}
		if !slices.Equal(got, tc.want) {
			t.Errorf("scroll %d: got %q, want %q", tc.scroll, got, tc.want)
		}
	}
}

func TestPanelWrapFitsMixedWidthAndKeepsText(t *testing.T) {
	for _, text := range []string{"You: 日本語の文章", "  [x] 話す　読む ", " > ● ちょこ", "word spaced 👩‍💻 and 記憶"} {
		for width := 2; width < 16; width++ {
			var out strings.Builder
			for _, row := range wrapPanelLine(line{raw(text)}, width) {
				rendered := renderGlyphs(row, width)
				if displayWidth(rendered) != width {
					t.Fatalf("%q at width %d yielded row %q with %d columns", text, width, rendered, displayWidth(rendered))
				}
				out.WriteString(rendered)
			}
			withoutSpaces := func(s string) string { return strings.Join(strings.Fields(s), "") }
			if got := withoutSpaces(out.String()); got != withoutSpaces(text) {
				t.Errorf("%q at width %d lost text: %q", text, width, got)
			}
		}
	}
}
