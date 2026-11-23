//go:build desktop && production

package desktop

import (
	"context"
	_ "embed"
	"runtime"

	"fyne.io/systray"
	"github.com/lxn/win"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

//go:embed icons/icon.ico
var iconICO []byte

var postThreadMessage = user32.NewProc("PostThreadMessageW")

func startTray(lifetime, ctx context.Context, window windowInvoker, title string, child *mascotChild) <-chan struct{} {
	done := make(chan struct{})
	go func() {
		// The tray must pump messages on the same thread that creates its
		// hidden Win32 window. Wails owns the main thread.
		runtime.LockOSThread()
		defer close(done)
		ready := make(chan struct{})
		var message win.MSG
		win.PeekMessage(&message, 0, 0, 0, 0) // Create this thread's message queue.
		thread := win.GetCurrentThreadId()
		go func() {
			select {
			case <-lifetime.Done():
			case <-done:
				return
			}
			select {
			case <-ready:
				systray.Quit()
			default:
				// A failed tray registration never calls onReady.
				postThreadMessage.Call(uintptr(thread), win.WM_QUIT, 0, 0)
			}
		}()
		systray.Run(func() {
			if lifetime.Err() != nil {
				return
			}
			systray.SetIcon(iconICO)
			systray.SetTooltip(title)
			showTitle, hideTitle := "Show", "Hide"
			if child != nil {
				showTitle, hideTitle = "Show Mascot", "Hide Mascot"
			}
			show := systray.AddMenuItem(showTitle, "").ClickedCh
			hide := systray.AddMenuItem(hideTitle, "").ClickedCh
			var showMain, hideMain <-chan struct{}
			if child != nil {
				showMain = systray.AddMenuItem("Show Main Window", "").ClickedCh
				hideMain = systray.AddMenuItem("Hide Main Window", "").ClickedCh
			}
			quit := systray.AddMenuItem("Quit", "").ClickedCh
			close(ready)
			if lifetime.Err() != nil {
				systray.Quit()
				return
			}
			// Systray waits for the ready callback to return before opening
			// the menu, so the click listener needs its own goroutine.
			go func() {
				for {
					select {
					case <-lifetime.Done():
						return
					case <-show:
						if child != nil {
							child.send("show")
						} else {
							window.Invoke(func() { wailsruntime.WindowShow(ctx) })
						}
					case <-hide:
						if child != nil {
							child.send("hide")
						} else {
							window.Invoke(func() { wailsruntime.WindowHide(ctx) })
						}
					case <-showMain:
						window.Invoke(func() { wailsruntime.WindowShow(ctx) })
					case <-hideMain:
						window.Invoke(func() { wailsruntime.WindowHide(ctx) })
					case <-quit:
						window.Invoke(func() { wailsruntime.Quit(ctx) })
						return
					}
				}
			}()
		}, nil)
	}()
	return done
}
