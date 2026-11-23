package tui

import (
	"context"
	"os"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"golang.org/x/sys/windows"
)

func userHomeDir() string {
	home, _ := windows.KnownFolderPath(windows.FOLDERID_Profile, 0)
	return home
}

// Bubble Tea v1 reads the initial Windows console size but has no Windows
// resize listener. Forward changes so the renderer and layout both reflow.
func watchTerminalResize(ctx context.Context, program *tea.Program) func() {
	ctx, cancel := context.WithCancel(ctx)
	done := make(chan struct{})
	go func() {
		defer close(done)
		ticker := time.NewTicker(250 * time.Millisecond)
		defer ticker.Stop()
		var last tea.WindowSizeMsg
		for {
			select {
			case <-ctx.Done():
				return
			case <-ticker.C:
				var info windows.ConsoleScreenBufferInfo
				if windows.GetConsoleScreenBufferInfo(windows.Handle(os.Stdout.Fd()), &info) != nil {
					continue
				}
				size := tea.WindowSizeMsg{
					Width:  int(info.Window.Right) - int(info.Window.Left) + 1,
					Height: int(info.Window.Bottom) - int(info.Window.Top) + 1,
				}
				if size != last {
					program.Send(size)
					last = size
				}
			}
		}
	}()
	return func() {
		cancel()
		<-done
	}
}
