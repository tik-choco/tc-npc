package vision

import (
	"fmt"
	"image"
	"path/filepath"
	"runtime"
	"strings"
	"unsafe"

	"github.com/kbinani/screenshot"
	"github.com/lxn/win"
	"golang.org/x/sys/windows"
)

var (
	user32              = windows.NewLazySystemDLL("user32.dll")
	getWindowDC         = user32.NewProc("GetWindowDC")
	getWindowTextLength = user32.NewProc("GetWindowTextLengthW")
	getWindowText       = user32.NewProc("GetWindowTextW")
	printWindow         = user32.NewProc("PrintWindow")
	setThreadDPIContext = user32.NewProc("SetThreadDpiAwarenessContext")
	enumWindowCallback  = windows.NewCallback(findWindow)
)

func primaryDisplayBounds() image.Rectangle {
	var first image.Rectangle
	for i, n := 0, screenshot.NumActiveDisplays(); i < n; i++ {
		bounds := screenshot.GetDisplayBounds(i)
		if bounds.Empty() {
			continue
		}
		if first.Empty() {
			first = bounds
		}
		// Win32 anchors the primary monitor at the virtual desktop origin.
		if bounds.Min == (image.Point{}) {
			return bounds
		}
	}
	return first
}

type windowSearch struct {
	app     bool
	matches func(string) bool
	hwnd    win.HWND
	frame   win.RECT
	class   string
}

func findWindow(hwnd win.HWND, search *windowSearch) uintptr {
	if !win.IsWindowVisible(hwnd) || win.IsIconic(hwnd) {
		return 1
	}

	var pid uint32
	win.GetWindowThreadProcessId(hwnd, &pid)
	// Querying the title of our own window can block on its message loop.
	if pid == windows.GetCurrentProcessId() {
		return 1
	}

	var classBuf [256]uint16
	n, _ := win.GetClassName(hwnd, &classBuf[0], len(classBuf))
	class := windows.UTF16ToString(classBuf[:n])
	if class == "" || class == "Progman" || class == "Button" {
		return 1
	}
	if win.GetWindowLong(hwnd, win.GWL_EXSTYLE)&win.WS_EX_TOOLWINDOW != 0 &&
		class != "Shell_TrayWnd" && windowTitle(hwnd) == "" {
		return 1
	}
	var cloaked uint32
	if windows.DwmGetWindowAttribute(windows.HWND(hwnd), windows.DWMWA_CLOAKED, unsafe.Pointer(&cloaked), 4) == nil && cloaked != 0 {
		return 1
	}

	var frame win.RECT
	err := windows.DwmGetWindowAttribute(windows.HWND(hwnd), windows.DWMWA_EXTENDED_FRAME_BOUNDS,
		unsafe.Pointer(&frame), uint32(unsafe.Sizeof(frame)))
	if err != nil && !win.GetWindowRect(hwnd, &frame) {
		return 1
	}
	if frame.Right <= frame.Left || frame.Bottom <= frame.Top {
		return 1
	}

	var name string
	if search.app {
		name = windowAppName(pid)
	} else {
		name = windowTitle(hwnd)
	}
	if !search.matches(name) {
		return 1
	}
	search.hwnd, search.frame, search.class = hwnd, frame, class
	return 0
}

func windowTitle(hwnd win.HWND) string {
	n, _, _ := getWindowTextLength.Call(uintptr(hwnd))
	if n == 0 {
		return ""
	}
	buf := make([]uint16, n+1)
	getWindowText.Call(uintptr(hwnd), uintptr(unsafe.Pointer(&buf[0])), uintptr(len(buf)))
	return windows.UTF16ToString(buf)
}

func windowAppName(pid uint32) string {
	process, err := windows.OpenProcess(windows.PROCESS_QUERY_LIMITED_INFORMATION, false, pid)
	if err != nil {
		return ""
	}
	defer windows.CloseHandle(process)
	path := make([]uint16, 32768)
	length := uint32(len(path))
	if err := windows.QueryFullProcessImageName(process, 0, &path[0], &length); err != nil {
		return ""
	}
	filename := windows.UTF16ToString(path[:length])
	if name := fileDescription(filename); name != "" {
		return name
	}
	return filepath.Base(filename)
}

func fileDescription(filename string) string {
	var handle windows.Handle
	size, err := windows.GetFileVersionInfoSize(filename, &handle)
	if err != nil || size == 0 {
		return ""
	}
	data := make([]byte, size)
	block := unsafe.Pointer(&data[0])
	if windows.GetFileVersionInfo(filename, 0, size, block) != nil {
		return ""
	}
	type translation struct{ language, codePage uint16 }
	var table *translation
	var n uint32
	if windows.VerQueryValue(block, `\VarFileInfo\Translation`, unsafe.Pointer(&table), &n) != nil {
		return ""
	}
	translations := unsafe.Slice(table, int(n)/int(unsafe.Sizeof(translation{})))
	for _, key := range []string{"FileDescription", "ProductName", "ProductShortName", "InternalName", "OriginalFilename"} {
		for _, entry := range translations {
			subBlock := fmt.Sprintf(`\StringFileInfo\%04x%04x\%s`, entry.language, entry.codePage, key)
			var value *uint16
			var length uint32
			if windows.VerQueryValue(block, subBlock, unsafe.Pointer(&value), &length) == nil && length > 0 {
				if name := strings.TrimSpace(windows.UTF16ToString(unsafe.Slice(value, length))); name != "" {
					return name
				}
			}
		}
	}
	return ""
}

func captureWindow(target string) (*image.RGBA, error) {
	runtime.LockOSThread()
	defer runtime.UnlockOSThread()
	// Keep GetWindowRect and the DWM frame bounds in physical pixels.
	if setThreadDPIContext.Find() == nil {
		const perMonitorAwareV2 = ^uintptr(3) // DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2 (-4)
		if previous, _, _ := setThreadDPIContext.Call(perMonitorAwareV2); previous != 0 {
			defer setThreadDPIContext.Call(previous)
		}
	}

	app, matches := windowMatcher(target)
	search := windowSearch{app: app, matches: matches}
	err := windows.EnumWindows(enumWindowCallback, unsafe.Pointer(&search))
	if search.hwnd == 0 {
		if err != nil {
			return nil, fmt.Errorf("enumerate windows: %w", err)
		}
		return nil, fmt.Errorf("visible window not found for target '%s'", target)
	}
	img, err := renderWindow(search.hwnd, search.frame, search.class == "#32770")
	if err != nil {
		return nil, fmt.Errorf("failed to capture window: %w", err)
	}
	return img, nil
}

func renderWindow(hwnd win.HWND, frame win.RECT, dialog bool) (*image.RGBA, error) {
	var rect win.RECT
	if !win.GetWindowRect(hwnd, &rect) {
		return nil, fmt.Errorf("GetWindowRect failed")
	}
	width, height := rect.Right-rect.Left, rect.Bottom-rect.Top
	if width <= 0 || height <= 0 {
		return nil, fmt.Errorf("window has empty bounds")
	}
	dc, _, _ := getWindowDC.Call(uintptr(hwnd))
	if dc == 0 {
		return nil, fmt.Errorf("GetWindowDC failed")
	}
	defer win.ReleaseDC(hwnd, win.HDC(dc))
	memory := win.CreateCompatibleDC(win.HDC(dc))
	if memory == 0 {
		return nil, fmt.Errorf("CreateCompatibleDC failed")
	}
	defer win.DeleteDC(memory)

	header := win.BITMAPINFOHEADER{
		BiSize: uint32(unsafe.Sizeof(win.BITMAPINFOHEADER{})), BiWidth: width, BiHeight: -height,
		BiPlanes: 1, BiBitCount: 32, BiCompression: win.BI_RGB,
	}
	var pixels unsafe.Pointer
	bitmap := win.CreateDIBSection(memory, &header, win.DIB_RGB_COLORS, &pixels, 0, 0)
	if bitmap == 0 {
		return nil, fmt.Errorf("CreateDIBSection failed")
	}
	defer win.DeleteObject(win.HGDIOBJ(bitmap))
	previous := win.SelectObject(memory, win.HGDIOBJ(bitmap))
	if previous == 0 || previous == ^win.HGDIOBJ(0) {
		return nil, fmt.Errorf("SelectObject failed")
	}
	defer win.SelectObject(memory, previous)

	// PrintWindow asks the application to render even when another window
	// covers it. Some applications only support the older GDI capture paths.
	printed := false
	for _, flags := range []uintptr{2, 0, 4} {
		if ok, _, _ := printWindow.Call(uintptr(hwnd), uintptr(memory), flags); ok != 0 {
			printed = true
			break
		}
	}
	if !printed && !win.BitBlt(memory, 0, 0, width, height, win.HDC(dc), 0, 0, win.SRCCOPY) {
		return nil, fmt.Errorf("PrintWindow and BitBlt failed")
	}
	win.GdiFlush()

	// DWM excludes the invisible resize border. Standard dialogs already
	// render without the border, so their content starts at the origin.
	offset := image.Pt(int(frame.Left-rect.Left), int(frame.Top-rect.Top))
	if dialog {
		offset = image.Point{}
	}
	size := image.Pt(int(frame.Right-frame.Left), int(frame.Bottom-frame.Top))
	clip := (image.Rectangle{Min: offset, Max: offset.Add(size)}).Intersect(image.Rect(0, 0, int(width), int(height)))
	if clip.Empty() {
		return nil, fmt.Errorf("window frame lies outside capture bounds")
	}
	img := image.NewRGBA(image.Rect(0, 0, clip.Dx(), clip.Dy()))
	src := unsafe.Slice((*byte)(pixels), int(width)*int(height)*4)
	for y := 0; y < clip.Dy(); y++ {
		for x := 0; x < clip.Dx(); x++ {
			s := ((y+clip.Min.Y)*int(width) + x + clip.Min.X) * 4
			d := y*img.Stride + x*4
			img.Pix[d], img.Pix[d+1], img.Pix[d+2], img.Pix[d+3] = src[s+2], src[s+1], src[s], 255
		}
	}
	return img, nil
}
