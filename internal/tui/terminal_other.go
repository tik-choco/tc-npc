//go:build !windows

package tui

import (
	"context"
	"os"
	"os/user"

	tea "github.com/charmbracelet/bubbletea"
)

func userHomeDir() string {
	if home, err := os.UserHomeDir(); err == nil {
		return home
	}
	if account, err := user.Current(); err == nil {
		return account.HomeDir
	}
	return ""
}

func watchTerminalResize(context.Context, *tea.Program) func() { return func() {} }
