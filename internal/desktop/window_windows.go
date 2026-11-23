//go:build desktop && production

package desktop

import (
	"context"
	"encoding/binary"
	"errors"
	"fmt"
	"log/slog"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"runtime"
	"sync"
	"sync/atomic"
	"unsafe"

	"github.com/lxn/win"
	"github.com/wailsapp/go-webview2/pkg/edge"
	"github.com/wailsapp/wails/v2"
	"github.com/wailsapp/wails/v2/pkg/options"
	"github.com/wailsapp/wails/v2/pkg/options/assetserver"
	winoptions "github.com/wailsapp/wails/v2/pkg/options/windows"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
	"golang.org/x/sys/windows"
)

type windowMode struct {
	title, stateKey, legacyLabel string
	width, height                int
	mascot, managed              bool
}

func runWindow(target url.URL, dataDir string, mode windowMode, child *mascotChild) error {
	lifetime, cancel := context.WithCancel(context.Background())
	defer cancel()
	statePath := filepath.Join(dataDir, "desktop-"+mode.stateKey+".json")
	state, err := readWindowState(statePath)
	if errors.Is(err, os.ErrNotExist) {
		if dir, dirErr := os.UserConfigDir(); dirErr == nil {
			state, err = readTauriWindowState(filepath.Join(dir, "local.tc-npc.app", ".window-state.json"), mode.legacyLabel)
		}
	}
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		slog.Warn("desktop: restore window state", "window", mode.stateKey, "error", err)
	}

	opts := &options.App{
		Title: mode.title, Width: mode.width, Height: mode.height,
		StartHidden: true, Frameless: mode.mascot, AlwaysOnTop: mode.mascot,
		DragAndDrop: &options.DragAndDrop{DisableWebViewDrop: true},
		Windows: &winoptions.Options{
			WebviewUserDataPath:               filepath.Join(dataDir, "webview2"),
			WebviewIsTransparent:              mode.mascot,
			WindowIsTranslucent:               mode.mascot,
			DisableFramelessWindowDecorations: mode.mascot,
			BackdropType:                      winoptions.None,
		},
		AssetServer: &assetserver.Options{
			Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.URL.Path != "/" {
					http.NotFound(w, r)
					return
				}
				w.Header().Set("Content-Type", "text/html; charset=utf-8")
				_, _ = w.Write([]byte("<!doctype html><html><head><title>tc-npc</title></head><body></body></html>"))
			}),
		},
	}
	if mode.mascot {
		// WebView2 only composites with alpha exactly zero.
		opts.BackgroundColour = options.NewRGBA(0, 0, 0, 0)
	}

	var (
		bootstrap sync.Once
		save      sync.Once
		hwnd      atomic.Uintptr
		icon      win.HICON
		trayDone  <-chan struct{}
		script    *scriptRegistration
	)
	startupError := make(chan error, 1)
	opts.OnDomReady = func(ctx context.Context) {
		bootstrap.Do(func() {
			fail := func(err error) {
				startupError <- err
				wailsruntime.Quit(ctx)
			}
			window, handle, chromium, err := wailsWindow(ctx)
			if err != nil {
				fail(err)
				return
			}
			if !window.Invoke(func() {
				if lifetime.Err() != nil {
					return
				}
				if mode.mascot {
					hideFromTaskbar(handle)
				}
				restoreGeometry(handle, mode, state)
				icon = setWindowIcon(handle)
				hwnd.Store(uintptr(handle))
				installGestures(chromium, handle, target, mode.mascot)

				// The bootstrap completed Wails' own first-navigation
				// setup. The remote page uses only the scoped gesture bridge.
				var shown sync.Once
				chromium.NavigationCompletedCallback = func(*edge.ICoreWebView2, *edge.ICoreWebView2NavigationCompletedEventArgs) {
					shown.Do(func() {
						if lifetime.Err() != nil {
							return
						}
						wailsruntime.WindowShow(ctx)
						if mode.managed {
							go readMascotCommands(lifetime, ctx, window)
						} else {
							trayDone = startTray(lifetime, ctx, window, mode.title, child)
						}
					})
				}
				script, err = navigateWithInit(chromium, target, mode.mascot, fail)
				if err != nil {
					fail(err)
				}
			}) && lifetime.Err() == nil {
				fail(fmt.Errorf("dispatch desktop initialization to the Wails UI thread"))
			}
		})
	}
	opts.OnBeforeClose = func(context.Context) bool {
		cancel()
		save.Do(func() {
			if handle := win.HWND(hwnd.Load()); handle != 0 {
				if current, ok := captureGeometry(handle); ok {
					if err := writeWindowState(statePath, current); err != nil {
						slog.Warn("desktop: save window state", "window", mode.stateKey, "error", err)
					}
				}
			}
		})
		return false
	}

	err = wails.Run(opts)
	cancel()
	runtime.KeepAlive(script)
	if icon != 0 {
		win.DestroyIcon(icon)
	}
	if trayDone != nil {
		<-trayDone
	}
	select {
	case err := <-startupError:
		return err
	default:
	}
	if err != nil {
		return fmt.Errorf("run %s window: %w", mode.title, err)
	}
	return nil
}

var user32 = windows.NewLazySystemDLL("user32.dll")

var (
	monitorFromRect          = user32.NewProc("MonitorFromRect")
	createIconFromResourceEx = user32.NewProc("CreateIconFromResourceEx")
)

func hideFromTaskbar(hwnd win.HWND) {
	style := win.GetWindowLong(hwnd, win.GWL_EXSTYLE)
	win.SetWindowLong(hwnd, win.GWL_EXSTYLE, (style&^win.WS_EX_APPWINDOW)|win.WS_EX_TOOLWINDOW)
	win.SetWindowPos(hwnd, 0, 0, 0, 0, 0, win.SWP_NOSIZE|win.SWP_NOMOVE|win.SWP_NOZORDER|win.SWP_NOACTIVATE|win.SWP_FRAMECHANGED)
}

func restoreGeometry(hwnd win.HWND, mode windowMode, state windowState) {
	var outer, client win.RECT
	if !win.GetWindowRect(hwnd, &outer) || !win.GetClientRect(hwnd, &client) {
		return
	}
	frameWidth := outer.Right - outer.Left - client.Right
	frameHeight := outer.Bottom - outer.Top - client.Bottom
	x, y := outer.Left, outer.Top
	dpi := max(96, int(win.GetDpiForWindow(hwnd)))
	width, height := int32(mode.width*dpi/96)+frameWidth, int32(mode.height*dpi/96)+frameHeight
	if state.valid() {
		width, height = state.Width+frameWidth, state.Height+frameHeight
		rect := win.RECT{Left: state.X, Top: state.Y, Right: state.X + width, Bottom: state.Y + height}
		if monitor, _, _ := monitorFromRect.Call(uintptr(unsafe.Pointer(&rect)), win.MONITOR_DEFAULTTONULL); monitor != 0 {
			x, y = state.X, state.Y
		}
	}
	win.SetWindowPos(hwnd, 0, x, y, width, height, win.SWP_NOZORDER|win.SWP_NOACTIVATE)
}

func captureGeometry(hwnd win.HWND) (windowState, bool) {
	const smCXPaddedBorder = 92
	var outer, client win.RECT
	if !win.GetWindowRect(hwnd, &outer) || !win.GetClientRect(hwnd, &client) {
		return windowState{}, false
	}
	if win.IsIconic(hwnd) || win.IsZoomed(hwnd) {
		// WINDOWPLACEMENT retains the normal bounds across minimize/maximize.
		var placement win.WINDOWPLACEMENT
		placement.Length = uint32(unsafe.Sizeof(placement))
		if !win.GetWindowPlacement(hwnd, &placement) {
			return windowState{}, false
		}
		normal := placement.RcNormalPosition
		x, y := normal.Left, normal.Top
		decorated := win.GetWindowLong(hwnd, win.GWL_EXSTYLE)&win.WS_EX_TOOLWINDOW == 0
		if decorated {
			var info win.MONITORINFO
			info.CbSize = uint32(unsafe.Sizeof(info))
			monitor := win.MonitorFromWindow(hwnd, win.MONITOR_DEFAULTTONEAREST)
			if win.GetMonitorInfo(monitor, &info) {
				x += info.RcWork.Left - info.RcMonitor.Left
				y += info.RcWork.Top - info.RcMonitor.Top
			}
		}
		width, height := normal.Right-normal.Left, normal.Bottom-normal.Top
		if decorated {
			dpi := win.GetDpiForWindow(hwnd)
			padding := win.GetSystemMetricsForDpi(smCXPaddedBorder, dpi)
			width -= 2 * (win.GetSystemMetricsForDpi(win.SM_CXSIZEFRAME, dpi) + padding)
			height -= 2*(win.GetSystemMetricsForDpi(win.SM_CYSIZEFRAME, dpi)+padding) + win.GetSystemMetricsForDpi(win.SM_CYCAPTION, dpi)
		}
		state := windowState{X: x, Y: y, Width: width, Height: height}
		return state, state.valid()
	}
	state := windowState{X: outer.Left, Y: outer.Top, Width: client.Right, Height: client.Bottom}
	return state, state.valid()
}

func setWindowIcon(hwnd win.HWND) win.HICON {
	// CreateIconFromResourceEx takes the image entry of an ICO, following
	// the file header and its directory. The embedded file has one entry.
	size := int(binary.LittleEndian.Uint32(iconICO[14:18]))
	offset := int(binary.LittleEndian.Uint32(iconICO[18:22]))
	icon, _, err := createIconFromResourceEx.Call(uintptr(unsafe.Pointer(&iconICO[offset])), uintptr(size), 1, 0x00030000, 0, 0, 0)
	if icon == 0 {
		slog.Warn("desktop: set window icon", "error", err)
		return 0
	}
	win.SendMessage(hwnd, win.WM_SETICON, 0, icon)
	win.SendMessage(hwnd, win.WM_SETICON, 1, icon)
	return win.HICON(icon)
}
