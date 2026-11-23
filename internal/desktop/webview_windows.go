//go:build desktop && production

package desktop

import (
	"context"
	"encoding/json"
	"fmt"
	"net/url"
	"reflect"
	"strings"
	"sync/atomic"
	"unsafe"

	"github.com/lxn/win"
	"github.com/wailsapp/go-webview2/pkg/edge"
	"golang.org/x/sys/windows"
)

// Copied verbatim from src/desktop/mod.rs. Existing page preferences win.
const pageDefaultsScript = `
(function () {
  try {
    var defaults = {
      'tc-npc:avatar-window:backdrop': 'transparent',
      'tc-npc:avatar-window:captions': 'bubble'
    };
    for (var key in defaults) {
      if (localStorage.getItem(key) === null) {
        localStorage.setItem(key, defaults[key]);
      }
    }
  } catch (e) {
    // Storage unavailable -- the page falls back to its own defaults, which
    // only costs an opaque window with a subtitle bar.
  }
})();
`

const gestureScript = `
  const send = message => window.chrome.webview.postMessage(message);
  const resizeEdge = e => {
    if (!resizable) return "";
    const margin = 5;
    const v = e.clientY < margin ? "n" : e.clientY >= innerHeight - margin ? "s" : "";
    const h = e.clientX < margin ? "w" : e.clientX >= innerWidth - margin ? "e" : "";
    return v + h;
  };
  document.addEventListener("mousedown", e => {
    if (!e.isTrusted || e.button !== 0 || !(e.target instanceof Element)) return;
    const edge = resizeEdge(e);
    if (edge) {
      e.preventDefault();
      send("tc-npc:resize:" + edge);
      return;
    }
    if (e.target.closest('button,a,input,select,textarea,label,[contenteditable]:not([contenteditable="false"]),[role="button"]')) return;
    const handle = e.target.closest("[data-tauri-drag-region]");
    if (handle && (handle === e.target || handle.getAttribute("data-tauri-drag-region") === "deep")) {
      e.preventDefault();
      send("tc-npc:drag");
    }
  }, true);
  if (resizable) {
    document.addEventListener("mousemove", e => {
      const edge = resizeEdge(e);
      if (edge) document.documentElement.style.cursor = edge + "-resize";
      else document.documentElement.style.removeProperty("cursor");
    }, true);
    document.addEventListener("mouseleave", () => document.documentElement.style.removeProperty("cursor"));
  }
`

func webviewOrigin(target url.URL) string {
	return target.Scheme + "://" + strings.TrimSuffix(target.Host, ":80")
}

func initializationScript(target url.URL, mascot bool) string {
	origin, _ := json.Marshal(webviewOrigin(target))
	script := "(function () {\n  if (self !== top || location.origin !== " + string(origin) + ") return;\n"
	if mascot {
		script += pageDefaultsScript
	}
	return script + fmt.Sprintf("  const resizable = %t;\n", mascot) + gestureScript + "})();"
}

type windowInvoker interface {
	Invoke(func()) bool
}

// Wails v2 exposes neither the HWND nor a document-created script hook.
// Keep the private frontend access in one checked adapter for the pinned
// Windows version. Wails still owns the window, WebView2, and message loop.
func wailsWindow(ctx context.Context) (windowInvoker, win.HWND, *edge.Chromium, error) {
	const frontendPackage = "github.com/wailsapp/wails/v2/internal/frontend/desktop/windows"
	unavailable := fmt.Errorf("Wails Windows frontend layout changed; cannot initialize desktop WebView2")
	frontend := reflect.ValueOf(ctx.Value("frontend"))
	if frontend.Kind() != reflect.Pointer || frontend.IsNil() ||
		frontend.Type().Elem().PkgPath() != frontendPackage || frontend.Type().Elem().Name() != "Frontend" {
		return nil, 0, nil, unavailable
	}
	window := frontend.Elem().FieldByName("mainWindow")
	chromium := frontend.Elem().FieldByName("chromium")
	if window.Kind() != reflect.Pointer || window.IsNil() ||
		window.Type().Elem().PkgPath() != frontendPackage || window.Type().Elem().Name() != "Window" ||
		!chromium.IsValid() || chromium.Type() != reflect.TypeFor[*edge.Chromium]() || chromium.IsNil() {
		return nil, 0, nil, unavailable
	}
	nativeWindow := reflect.NewAt(window.Type().Elem(), window.UnsafePointer())
	invoker, ok := nativeWindow.Interface().(windowInvoker)
	handle := nativeWindow.MethodByName("Handle")
	if !ok || !handle.IsValid() || handle.Type().NumIn() != 0 || handle.Type().NumOut() != 1 ||
		handle.Type().Out(0).Kind() != reflect.Uintptr {
		return nil, 0, nil, unavailable
	}
	return invoker, win.HWND(handle.Call(nil)[0].Uint()), (*edge.Chromium)(chromium.UnsafePointer()), nil
}

func installGestures(chromium *edge.Chromium, hwnd win.HWND, target url.URL, mascot bool) {
	origin := webviewOrigin(target)
	previous := chromium.MessageCallback
	chromium.MessageCallback = func(message string, sender *edge.ICoreWebView2, args *edge.ICoreWebView2WebMessageReceivedEventArgs) {
		hit, allowed := gestureHits[message]
		if !allowed {
			// Wails continues rejecting messages from the remote origin.
			previous(message, sender, args)
			return
		}
		if (!mascot && hit != win.HTCAPTION) || win.IsZoomed(hwnd) {
			return
		}
		topSource, topErr := sender.GetSource()
		frameSource, frameErr := args.GetSource()
		if topErr != nil || frameErr != nil || !hasOrigin(topSource, origin) || !hasOrigin(frameSource, origin) {
			return
		}
		var cursor win.POINT
		win.GetCursorPos(&cursor)
		position := uintptr(uint16(cursor.X)) | uintptr(uint16(cursor.Y))<<16
		win.ReleaseCapture()
		win.PostMessage(hwnd, win.WM_NCLBUTTONDOWN, hit, position)
	}
}

var gestureHits = map[string]uintptr{
	"tc-npc:drag":      win.HTCAPTION,
	"tc-npc:resize:n":  win.HTTOP,
	"tc-npc:resize:ne": win.HTTOPRIGHT,
	"tc-npc:resize:e":  win.HTRIGHT,
	"tc-npc:resize:se": win.HTBOTTOMRIGHT,
	"tc-npc:resize:s":  win.HTBOTTOM,
	"tc-npc:resize:sw": win.HTBOTTOMLEFT,
	"tc-npc:resize:w":  win.HTLEFT,
	"tc-npc:resize:nw": win.HTTOPLEFT,
}

func hasOrigin(source, origin string) bool {
	uri, err := url.Parse(source)
	return err == nil && uri.User == nil && webviewOrigin(*uri) == origin
}

// navigateWithInit runs on the WebView2 apartment thread. The COM method
// completes asynchronously, so navigating before the callback could let the
// page read stale defaults. Both documents and their reloads keep the real
// HTTP origin for storage, fetches, and WebSockets.
func navigateWithInit(chromium *edge.Chromium, target url.URL, mascot bool, fail func(error)) (*scriptRegistration, error) {
	view, err := chromium.GetController().GetCoreWebView2()
	if err != nil {
		return nil, fmt.Errorf("get desktop WebView2: %w", err)
	}
	defer view.Release() // The Chromium instance owns its own reference.
	script, err := windows.UTF16PtrFromString(initializationScript(target, mascot))
	if err != nil {
		return nil, fmt.Errorf("encode WebView2 initialization script: %w", err)
	}

	// The edge binding has an unexported completion handler type. Use its
	// named vtable slot with the native HRESULT/LPCWSTR callback ABI.
	vtbl := reflect.ValueOf(view).Elem().FieldByName("vtbl")
	if vtbl.Kind() != reflect.Pointer || vtbl.IsNil() {
		return nil, fmt.Errorf("WebView2 vtable layout changed")
	}
	method := vtbl.Elem().FieldByName("AddScriptToExecuteOnDocumentCreated")
	if method.Kind() != reflect.Uintptr || method.Uint() == 0 {
		return nil, fmt.Errorf("WebView2 document-created method is unavailable")
	}
	handler := &scriptRegistration{vtbl: &scriptRegistrationVTable, done: func(status int32) {
		if status < 0 {
			fail(fmt.Errorf("register WebView2 initialization script: HRESULT 0x%08x", uint32(status)))
			return
		}
		if err := view.Navigate(target.String()); err != nil {
			fail(fmt.Errorf("navigate to %s: %w", target.String(), err))
		}
	}}
	handler.refs.Store(1)
	status, _, _ := edge.ComProc(method.Uint()).Call(
		uintptr(unsafe.Pointer(view)), uintptr(unsafe.Pointer(script)), uintptr(unsafe.Pointer(handler)),
	)
	if int32(status) < 0 {
		return nil, fmt.Errorf("add WebView2 initialization script: HRESULT 0x%08x", uint32(status))
	}
	return handler, nil
}

type scriptRegistration struct {
	vtbl *scriptCallbackVTable
	refs atomic.Uint32
	done func(status int32)
}

type scriptCallbackVTable struct {
	queryInterface, addRef, release, invoke uintptr
}

var (
	unknownIID       = windows.GUID{Data4: [8]byte{0xC0, 0, 0, 0, 0, 0, 0, 0x46}}
	scriptHandlerIID = windows.GUID{Data1: 0xB99369F3, Data2: 0x9B11, Data3: 0x47B5,
		Data4: [8]byte{0xBC, 0x6F, 0x8E, 0x78, 0x95, 0xFC, 0xEA, 0x17}}
	scriptRegistrationVTable = scriptCallbackVTable{
		queryInterface: windows.NewCallback(func(self *scriptRegistration, iid *windows.GUID, out **scriptRegistration) uintptr {
			if out == nil {
				return 0x80004003 // E_POINTER
			}
			*out = nil
			if iid == nil || (*iid != unknownIID && *iid != scriptHandlerIID) {
				return 0x80004002 // E_NOINTERFACE
			}
			*out = self
			self.refs.Add(1)
			return 0
		}),
		addRef:  windows.NewCallback(func(self *scriptRegistration) uintptr { return uintptr(self.refs.Add(1)) }),
		release: windows.NewCallback(func(self *scriptRegistration) uintptr { return uintptr(self.refs.Add(^uint32(0))) }),
		invoke: windows.NewCallback(func(self *scriptRegistration, status uintptr, _ *uint16) uintptr {
			self.done(int32(status))
			return 0
		}),
	}
)
