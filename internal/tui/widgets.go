package tui

import (
	"strings"
	"unicode"

	"github.com/charmbracelet/lipgloss"
	"github.com/mattn/go-runewidth"
)

// Match the reference's non-CJK width policy for ambiguous characters.
var cells = runewidth.Condition{StrictEmojiNeutral: true}

func displayWidth(s string) int { return cells.StringWidth(s) }

func truncateDisplay(s string, width int) string {
	if width <= 0 {
		return ""
	}
	if displayWidth(s) <= width {
		return s
	}
	tail := ""
	if width > 3 {
		tail = "..."
		width -= 3
	}
	if width != 1 {
		return cells.Truncate(s, width+displayWidth(tail), tail)
	}
	// A two-cell glyph can never fit in a one-cell budget. Skip it so a
	// narrow glyph later in the string can still use the available cell.
	var out strings.Builder
	used := 0
	for glyph := range strings.SplitSeq(cells.Wrap(s, 1), "\n") {
		w := displayWidth(glyph)
		if w > width {
			continue
		}
		if used+w > width {
			break
		}
		out.WriteString(glyph)
		used += w
	}
	return out.String() + tail
}

func padDisplayWidth(s string, width int) string {
	return s + strings.Repeat(" ", max(0, width-displayWidth(s)))
}

// wrapLines normalizes whitespace. Overlong words still need hard breaks
// because Japanese sentences routinely contain no spaces.
func wrapLines(s string, width int) []string {
	width = max(width, 1)
	words := strings.Fields(s)
	if len(words) == 0 {
		return []string{""}
	}
	var lines []string
	var current strings.Builder
	used := 0
	for _, word := range words {
		w := displayWidth(word)
		if current.Len() > 0 {
			if used+1+w > width {
				lines = append(lines, current.String())
				current.Reset()
				used = 0
			} else {
				current.WriteByte(' ')
				used++
			}
		}
		current.WriteString(word)
		used += w
	}
	lines = append(lines, current.String())
	return splitLongLines(lines, width)
}

func splitLongLines(lines []string, width int) []string {
	var out []string
	for _, line := range lines {
		if displayWidth(line) <= width {
			out = append(out, line)
			continue
		}
		for part := range strings.SplitSeq(cells.Wrap(line, width), "\n") {
			// A glyph that does not fit an empty row cannot be drawn.
			if part != "" && displayWidth(part) <= width {
				out = append(out, part)
			}
		}
	}
	return out
}

func cropLines(lines []string, height int) []string {
	return lines[max(0, len(lines)-height):]
}

func fitLines(lines []string, width, height int) []string {
	out := make([]string, height)
	for i, line := range cropLines(lines, height) {
		out[i] = truncateDisplay(line, width)
	}
	return out
}

type span struct {
	text  string
	style lipgloss.Style
}

type line []span

func raw(text string) span { return span{text: text} }

func chip(label string, color lipgloss.Color) span {
	return span{" " + label + " ", chipStyle(color)}
}

func node(label string, active bool) span {
	return span{" " + label + " ", nodeStyle(active)}
}

func edge(label string) span {
	if label == "" {
		return span{"-->", mutedStyle()}
	}
	return span{"--" + label + "-->", mutedStyle()}
}

// Protocol text is rendered as terminal cells. Strip controls before styling
// so embedded newlines or escape codes cannot move the terminal cursor.
func terminalText(s string) string {
	return strings.Map(func(r rune) rune {
		if unicode.IsControl(r) {
			return -1
		}
		return r
	}, s)
}

type glyph struct {
	text  string
	style *lipgloss.Style
	width int
}

// wrapPanelLine keeps the styles and whitespace needed for prefixes and
// badges. As in the reference's untrimmed wrapping, spaces up to a soft
// break are consumed by the preceding row.
func wrapPanelLine(spans line, width int) [][]glyph {
	var rows [][]glyph
	var current, spaces, word []glyph
	var lineWidth, spaceWidth, wordWidth int
	previousWasWord := false
	for i := range spans {
		// At one column Wrap separates the visible graphemes without
		// splitting joined emoji or combining characters into runes.
		for text := range strings.SplitSeq(cells.Wrap(terminalText(spans[i].text), 1), "\n") {
			if text == "" {
				continue
			}
			w := displayWidth(text)
			if w > width {
				continue
			}
			isSpace := strings.TrimFunc(text, func(r rune) bool {
				return (unicode.IsSpace(r) && r != '\u00a0') || r == '\u200b'
			}) == ""

			if (previousWasWord && isSpace) || (len(current) == 0 && spaceWidth+wordWidth+w > width) {
				current = append(current, spaces...)
				current = append(current, word...)
				lineWidth += spaceWidth + wordWidth
				spaces, word = spaces[:0], word[:0]
				spaceWidth, wordWidth = 0, 0
			}
			if lineWidth >= width || (w > 0 && lineWidth+spaceWidth+wordWidth+w > width) {
				rows = append(rows, current)
				remaining := width - lineWidth
				current, lineWidth = nil, 0
				for len(spaces) != 0 && spaces[0].width <= remaining {
					remaining -= spaces[0].width
					spaceWidth -= spaces[0].width
					spaces = spaces[1:]
				}
				if isSpace && len(spaces) == 0 {
					continue
				}
			}
			g := glyph{text, &spans[i].style, w}
			if isSpace {
				spaces = append(spaces, g)
				spaceWidth += w
			} else {
				word = append(word, g)
				wordWidth += w
			}
			previousWasWord = !isSpace
		}
	}
	if len(current) == 0 && len(word) == 0 && len(spaces) != 0 {
		rows = append(rows, nil)
	}
	current = append(current, spaces...)
	current = append(current, word...)
	if len(current) != 0 || len(rows) == 0 {
		rows = append(rows, current)
	}
	return rows
}

func renderGlyphs(glyphs []glyph, width int) string {
	var out, text strings.Builder
	used := 0
	for len(glyphs) > 0 {
		style := glyphs[0].style
		i := 0
		for i < len(glyphs) && glyphs[i].style == style {
			text.WriteString(glyphs[i].text)
			used += glyphs[i].width
			i++
		}
		out.WriteString(style.Render(text.String()))
		text.Reset()
		glyphs = glyphs[i:]
	}
	out.WriteString(strings.Repeat(" ", max(0, width-used)))
	return out.String()
}

func renderLine(spans line, width int) string {
	var row strings.Builder
	remaining := width
	for _, span := range spans {
		t := terminalText(span.text)
		part := cells.Truncate(t, remaining, "")
		row.WriteString(span.style.Render(part))
		remaining -= displayWidth(part)
		if part != t || remaining == 0 {
			break
		}
	}
	row.WriteString(strings.Repeat(" ", remaining))
	return row.String()
}
