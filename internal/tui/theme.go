package tui

import "github.com/charmbracelet/lipgloss"

const (
	colorPrimary   = lipgloss.Color("#5A67D8")
	colorSecondary = lipgloss.Color("#2DD4BF")
	colorSuccess   = lipgloss.Color("#04E384")
	colorWarning   = lipgloss.Color("#FFB800")
	colorError     = lipgloss.Color("#FF4B4B")
	colorGray      = lipgloss.Color("#3C3C3C")
	colorText      = lipgloss.Color("#E1E1E1")
	colorMuted     = lipgloss.Color("#666666")
	colorHighlight = lipgloss.Color("#FFFFFF")
)

func titleStyle(color lipgloss.Color) lipgloss.Style {
	return lipgloss.NewStyle().Foreground(color).Bold(true)
}

func mutedStyle() lipgloss.Style {
	return lipgloss.NewStyle().Foreground(colorMuted)
}

// Only badges have a background. Panels use the terminal's own background.
func chipStyle(color lipgloss.Color) lipgloss.Style {
	return titleStyle(colorHighlight).Background(color)
}

func nodeStyle(active bool) lipgloss.Style {
	color := colorGray
	if active {
		color = colorPrimary
	}
	return lipgloss.NewStyle().Foreground(colorHighlight).Background(color)
}

func borderStyle(focused bool) lipgloss.Style {
	if focused {
		return titleStyle(colorPrimary)
	}
	return lipgloss.NewStyle().Foreground(colorGray)
}

func activityColor(activity systemActivity) lipgloss.Color {
	return [...]lipgloss.Color{colorError, colorWarning, colorSecondary, colorSuccess}[activity]
}
