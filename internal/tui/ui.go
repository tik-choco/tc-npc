package tui

import (
	"strings"

	"github.com/charmbracelet/lipgloss"
)

const (
	compactCols    = 80
	compactRows    = 24
	minPanelHeight = 3
	eventLogMin    = 4
	eventLogMax    = 6
	chatPercent    = 62
)

func render(a *app, width, height int) string {
	if width <= 0 || height <= 0 {
		return ""
	}
	var rows []string
	if a.inConfig() {
		rows = renderConfig(a, width, height)
	} else {
		rows = renderMain(a, width, height)
	}
	return strings.Join(rows, "\n")
}

func renderMain(a *app, width, height int) []string {
	compact := width < compactCols || height < compactRows
	rows := make([]string, 0, height)
	rows = append(rows, renderLine(topBar(a, width), width))
	if height == 1 {
		return rows
	}
	if height == 2 {
		return append(rows, renderLine(footerLine(compact), width))
	}

	// Measure the fixed rows first. The scrollable panels absorb the
	// remaining height, even when the terminal shrinks during a session.
	remaining := height - 3 // top, input and footer
	if !compact {
		if pipeline := flowRow(a, width); pipeline != nil {
			rows = append(rows, renderLine(pipeline, width))
			remaining--
		}
	}
	errorCap := 4
	if compact {
		errorCap = 2
	}
	errors := errorLines(a, width, min(errorCap, remaining))
	for _, line := range errors {
		rows = append(rows, renderLine(line, width))
	}
	remaining -= len(errors)

	events := 0
	if !compact && remaining >= 14 {
		events = min(eventLogMax, max(eventLogMin, remaining/6))
	}
	rows = append(rows, renderPanels(a, width, remaining-events, compact)...)
	if events > 0 {
		rows = append(rows, renderEventLog(a, width, events)...)
	}
	return append(rows, renderLine(inputLine(a, width), width), renderLine(footerLine(compact), width))
}

func renderPanels(a *app, width, height int, compact bool) []string {
	switch {
	case height < minPanelHeight:
		rows := make([]string, height)
		for i := range rows {
			rows[i] = strings.Repeat(" ", width)
		}
		return rows
	case !compact:
		chatWidth := (width*chatPercent + 50) / 100
		return joinPanels(renderChat(a, chatWidth, height), renderLog(a, width-chatWidth, height))
	case height >= 2*minPanelHeight:
		chatHeight := (height*65 + 50) / 100
		return append(renderChat(a, width, chatHeight), renderLog(a, width, height-chatHeight)...)
	default:
		return renderChat(a, width, height)
	}
}

func joinPanels(left, right []string) []string {
	for i := range left {
		left[i] += right[i]
	}
	return left
}

func renderChat(a *app, width, height int) []string {
	lines := make([]line, 0, len(a.chat)+1)
	for _, row := range a.chat {
		switch {
		case row.silent:
			// Show silent turns explicitly so they cannot be mistaken for
			// dropped replies.
			lines = append(lines, line{{"(反応なし: " + row.text + ")", mutedStyle().Italic(true)}})
		case row.role == "assistant":
			lines = append(lines, line{{"NPC: ", titleStyle(colorPrimary)}, raw(row.text)})
		default:
			lines = append(lines, line{{"You: ", titleStyle(colorSecondary)}, raw(row.text)})
		}
	}
	if a.pending {
		lines = append(lines, line{{"NPC: ...", mutedStyle()}})
	}
	return renderPanel("チャット", a.focus == focusChat, lines, width, height, a.chatScroll)
}

func renderLog(a *app, width, height int) []string {
	lines := make([]line, 0, len(a.log))
	style := lipgloss.NewStyle().Foreground(colorWarning)
	for _, row := range a.log {
		lines = append(lines, line{{"[" + row.label + "] ", style}, raw(row.text)})
	}
	return renderPanel("ログ", a.focus == focusLog, lines, width, height, a.logScroll)
}

func renderEventLog(a *app, width, height int) []string {
	lines := make([]line, 0, len(a.eventLog))
	for _, event := range a.eventLog {
		lines = append(lines, line{{event, mutedStyle()}})
	}
	return renderPanel("イベント", false, lines, width, height, 0)
}

func renderPanel(title string, focused bool, lines []line, width, height, scrollUp int) []string {
	rows := make([]string, height)
	if width <= 0 || height <= 0 {
		return rows
	}
	border := borderStyle(focused)
	if width == 1 {
		rows[0] = border.Render("┌")
		for i := 1; i < height-1; i++ {
			rows[i] = border.Render("│")
		}
		if height > 1 {
			rows[height-1] = border.Render("└")
		}
		return rows
	}

	innerWidth := width - 2
	title = cells.Truncate(title, innerWidth, "")
	rows[0] = border.Render("┌") + title + border.Render(strings.Repeat("─", innerWidth-displayWidth(title))+"┐")
	if height > 1 {
		rows[height-1] = border.Render("└" + strings.Repeat("─", innerWidth) + "┘")
	}
	if height > 2 {
		var visible [][]glyph
		if innerWidth > 0 {
			visible = visibleRows(lines, innerWidth, height-2, scrollUp)
		}
		rail := border.Render("│")
		for i := range height - 2 {
			var row []glyph
			if i < len(visible) {
				row = visible[i]
			}
			rows[i+1] = rail + renderGlyphs(row, innerWidth) + rail
		}
	}
	return rows
}

// Count wrapped rows backwards from the most recent entry. Collect only the
// rows the viewport and scroll offset could use. Clamp an overscroll to the
// oldest page while leaving the app's offset intact.
func visibleRows(lines []line, width, height, scrollUp int) [][]glyph {
	var reversed [][]glyph
	target := height + scrollUp
	for i := len(lines) - 1; i >= 0 && len(reversed) < target; i-- {
		wrapped := wrapPanelLine(lines[i], width)
		for j := len(wrapped) - 1; j >= 0 && len(reversed) < target; j-- {
			reversed = append(reversed, wrapped[j])
		}
	}
	start := min(scrollUp, max(0, len(reversed)-height))
	end := min(len(reversed), start+height)
	rows := make([][]glyph, 0, end-start)
	for i := end - 1; i >= start; i-- {
		rows = append(rows, reversed[i])
	}
	return rows
}

func topBar(a *app, width int) line {
	activity := a.systemActivity()
	character := "(未設定)"
	if a.character != nil {
		character = a.character.Name
	}
	out := line{
		{"tc-npc", titleStyle(colorHighlight)},
		raw(" "),
		chip(activity.label(), activityColor(activity)),
		raw(" "),
		chip("キャラ: "+character, colorMuted),
	}
	if a.notice != "" {
		out = append(out, raw(" "), span{truncateDisplay(terminalText(a.notice), width/3), mutedStyle()})
	}
	return out
}

func flowRow(a *app, width int) line {
	var out line
	for i, module := range a.moduleStates() {
		if i > 0 {
			out = append(out, edge(""))
		}
		out = append(out, node(module.name, module.on))
	}
	used := 0
	for _, span := range out {
		used += displayWidth(span.text)
	}
	// A wrapped pipeline diagram would lose its ordering.
	if used > width {
		return nil
	}
	return out
}

func errorLines(a *app, width, height int) []line {
	if height == 0 {
		return nil
	}
	message := a.statusLine
	style := lipgloss.NewStyle()
	if a.operatorError != nil {
		message = "エラー: " + a.operatorError.message
		style = style.Foreground(colorError)
	}
	if message == "" {
		return nil
	}
	// Keep long errors from displacing the transcript.
	text := fitLines(wrapLines(message, width), width, height)
	out := make([]line, 0, height)
	for _, row := range text {
		out = append(out, line{{row, style}})
	}
	return out
}

func inputLine(a *app, width int) line {
	input := terminalText(a.input)
	if a.focus == focusInput {
		return line{{truncateDisplay("> "+input+"_", width), titleStyle(colorText)}}
	}
	return line{{truncateDisplay("  "+input, width), mutedStyle()}}
}

func footerLine(compact bool) line {
	hints := "Tab:フォーカス  F2:設定  Enter:送信  PgUp/PgDn:スクロール  Ctrl+C:終了"
	if compact {
		hints = "Tab:移動  F2:設定  Ctrl+C:終了"
	}
	return line{{hints, mutedStyle()}}
}

func renderConfig(a *app, width, height int) []string {
	rows := make([]string, 0, height)
	rows = append(rows, renderLine(topBar(a, width), width))
	if height > 2 {
		moduleWidth := (width + 1) / 2
		rows = append(rows, joinPanels(
			renderModules(a, moduleWidth, height-2),
			renderCharacters(a, width-moduleWidth, height-2),
		)...)
	}
	if height > 1 {
		rows = append(rows, renderLine(line{{"Tab/Esc:監視画面へ戻る  Ctrl+C:終了", mutedStyle()}}, width))
	}
	return rows
}

func renderModules(a *app, width, height int) []string {
	modules := a.moduleStates()
	nameWidth := 0
	for _, module := range modules {
		nameWidth = max(nameWidth, displayWidth(module.name))
	}
	var lines []line
	for _, module := range modules {
		mark := "[ ] "
		if module.on {
			mark = "[x] "
		}
		row := line{{mark, nodeStyle(module.on)}, raw(padDisplayWidth(module.name, nameWidth))}
		// These modules' startup flags determine which tasks exist.
		switch module.name {
		case "talk", "memory", "vision", "action":
			row = append(row, span{"  要再起動", mutedStyle()})
		}
		lines = append(lines, row)
	}
	return renderPanel("モジュール (表示のみ)", a.focus == focusModules, lines, width, height, 0)
}

func renderCharacters(a *app, width, height int) []string {
	focused := a.focus == focusCharacters
	lines := make([]line, 0, len(a.characters))
	for i, entry := range a.characters {
		cursor, mark := "  ", "○"
		if focused && i == a.characterCursor {
			cursor = "> "
		}
		if entry.active {
			mark = "●"
		}
		lines = append(lines, line{{cursor + mark + " ", nodeStyle(entry.active)}, raw(entry.name)})
	}
	if len(lines) == 0 {
		lines = append(lines, line{{"(取得中/なし)", mutedStyle()}})
	}
	return renderPanel("キャラクタ (Enter: 切替)", focused, lines, width, height, 0)
}
