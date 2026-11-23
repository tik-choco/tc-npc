package vision

import (
	"encoding/binary"
	"fmt"
	"image"
	"os"
	"strings"

	"github.com/jezek/xgb"
	"github.com/jezek/xgb/randr"
	"github.com/jezek/xgb/xinerama"
	"github.com/jezek/xgb/xproto"
	"github.com/kbinani/screenshot"
)

func primaryDisplayBounds() image.Rectangle {
	conn, err := xgb.NewConn()
	if err != nil {
		return image.Rectangle{}
	}
	defer conn.Close()
	bounds := randrDisplayBounds(conn)
	if bounds.Empty() {
		return screenshot.GetDisplayBounds(0)
	}
	if os.Getenv("XDG_SESSION_TYPE") == "wayland" {
		return bounds
	}

	// On X11, screenshot.CaptureRect measures from Xinerama screen zero.
	// RandR reports absolute root-window coordinates and the actual primary.
	if xinerama.Init(conn) == nil {
		screens, err := xinerama.QueryScreens(conn).Reply()
		if err == nil && len(screens.ScreenInfo) != 0 {
			first := screens.ScreenInfo[0]
			bounds = bounds.Sub(image.Pt(int(first.XOrg), int(first.YOrg)))
		}
	}
	return bounds
}

func randrDisplayBounds(conn *xgb.Conn) image.Rectangle {
	if randr.Init(conn) != nil {
		return image.Rectangle{}
	}
	reply, err := randr.GetMonitors(conn, xproto.Setup(conn).DefaultScreen(conn).Root, true).Reply()
	if err != nil || reply == nil {
		return image.Rectangle{}
	}
	var first image.Rectangle
	for _, monitor := range reply.Monitors {
		if len(monitor.Outputs) == 0 || monitor.Width == 0 || monitor.Height == 0 {
			continue
		}
		origin := image.Pt(int(monitor.X), int(monitor.Y))
		bounds := image.Rect(0, 0, int(monitor.Width), int(monitor.Height)).Add(origin)
		if monitor.Primary {
			return bounds
		}
		if first.Empty() {
			first = bounds
		}
	}
	return first
}

func captureWindow(target string) (*image.RGBA, error) {
	conn, err := xgb.NewConn()
	if err != nil {
		return nil, fmt.Errorf("connect to X11: %w", err)
	}
	defer conn.Close()
	root := xproto.Setup(conn).DefaultScreen(conn).Root
	list, err := x11Property(conn, root, x11Atom(conn, "_NET_CLIENT_LIST_STACKING"), xproto.AtomWindow)
	if err != nil {
		return nil, fmt.Errorf("enumerate windows: %w", err)
	}

	state := x11Atom(conn, "_NET_WM_STATE")
	hidden := x11Atom(conn, "_NET_WM_STATE_HIDDEN")
	title := x11Atom(conn, "_NET_WM_NAME")
	utf8 := x11Atom(conn, "UTF8_STRING")
	compound := x11Atom(conn, "COMPOUND_TEXT")
	app, matches := windowMatcher(target)
	// The window manager lists windows bottom-to-top. Try the topmost match.
	for i := len(list)/4 - 1; i >= 0; i-- {
		window := xproto.Window(xgb.Get32(list[i*4:]))
		if x11WindowHidden(conn, window, state, hidden) {
			continue
		}
		var value string
		if app {
			wmClass, _ := x11Property(conn, window, xproto.AtomWmClass, xproto.AtomString)
			parts := strings.SplitN(string(wmClass), "\x00", 3)
			if len(parts) >= 2 {
				value = parts[1]
			}
		} else {
			value = x11WindowTitle(conn, window, title, utf8, compound)
		}
		if matches(value) {
			img, err := captureX11Window(conn, window)
			if err != nil {
				return nil, fmt.Errorf("failed to capture window: %w", err)
			}
			return img, nil
		}
	}
	return nil, fmt.Errorf("visible window not found for target '%s'", target)
}

func x11Atom(conn *xgb.Conn, name string) xproto.Atom {
	result, err := xproto.InternAtom(conn, true, uint16(len(name)), name).Reply()
	if err != nil || result == nil {
		return xproto.AtomNone
	}
	return result.Atom
}

func x11Property(conn *xgb.Conn, window xproto.Window, property, kind xproto.Atom) ([]byte, error) {
	if property == xproto.AtomNone {
		return nil, nil
	}
	reply, err := xproto.GetProperty(conn, false, window, property, kind, 0, 1024).Reply()
	if err != nil || reply == nil {
		return nil, err
	}
	return reply.Value, nil
}

func x11WindowHidden(conn *xgb.Conn, window xproto.Window, state, hidden xproto.Atom) bool {
	states, _ := x11Property(conn, window, state, xproto.AtomAtom)
	for len(states) >= 4 {
		if xproto.Atom(xgb.Get32(states)) == hidden {
			return true
		}
		states = states[4:]
	}
	return false
}

func x11WindowTitle(conn *xgb.Conn, window xproto.Window, title, utf8, compound xproto.Atom) string {
	for window != 0 {
		value, _ := x11Property(conn, window, title, utf8)
		if len(value) == 0 && compound != xproto.AtomNone {
			value, _ = x11Property(conn, window, xproto.AtomWmName, compound)
		}
		if len(value) != 0 {
			return strings.ToValidUTF8(string(value), "\uFFFD")
		}
		tree, err := xproto.QueryTree(conn, window).Reply()
		if err != nil || tree == nil {
			break
		}
		window = tree.Parent
	}
	return ""
}

func captureX11Window(conn *xgb.Conn, window xproto.Window) (*image.RGBA, error) {
	geometry, err := xproto.GetGeometry(conn, xproto.Drawable(window)).Reply()
	if err != nil {
		return nil, err
	}
	width, height := int(geometry.Width), int(geometry.Height)
	reply, err := xproto.GetImage(conn, xproto.ImageFormatZPixmap, xproto.Drawable(window),
		0, 0, geometry.Width, geometry.Height, ^uint32(0)).Reply()
	if err != nil {
		return nil, err
	}

	setup := xproto.Setup(conn)
	for _, format := range setup.PixmapFormats {
		if format.Depth == reply.Depth {
			return decodeX11Pixels(reply.Data, width, height, format, setup.ImageByteOrder == xproto.ImageOrderLSBFirst)
		}
	}
	return nil, fmt.Errorf("missing pixmap format for depth %d", reply.Depth)
}

func decodeX11Pixels(data []byte, width, height int, format xproto.Format, littleEndian bool) (*image.RGBA, error) {
	switch format.Depth {
	case 8, 16, 24, 32:
	default:
		return nil, fmt.Errorf("unsupported X11 pixel depth %d", format.Depth)
	}
	bytesPerPixel := int(format.BitsPerPixel) / 8
	pad := int(format.ScanlinePad)
	if bytesPerPixel < int(format.Depth)/8 || pad < 8 || width <= 0 || height <= 0 {
		return nil, fmt.Errorf("invalid X11 pixmap geometry")
	}
	stride := (width*int(format.BitsPerPixel) + pad - 1) / pad * (pad / 8)
	if stride > len(data)/height {
		return nil, fmt.Errorf("incomplete X11 pixmap")
	}

	var order binary.ByteOrder = binary.BigEndian
	if littleEndian {
		order = binary.LittleEndian
	}
	img := image.NewRGBA(image.Rect(0, 0, width, height))
	for y := 0; y < height; y++ {
		for x := 0; x < width; x++ {
			s, d := y*stride+x*bytesPerPixel, y*img.Stride+x*4
			switch format.Depth {
			case 8:
				p := uint16(data[s])
				img.Pix[d], img.Pix[d+1], img.Pix[d+2] = byte((p>>6)*255/3), byte((p>>2&7)*255/7), byte((p&3)*255/3)
			case 16:
				p := order.Uint16(data[s:])
				img.Pix[d], img.Pix[d+1], img.Pix[d+2] = byte((p>>11)*255/31), byte((p>>5&63)*255/63), byte((p&31)*255/31)
			default:
				r, g, b := data[s], data[s+1], data[s+2]
				if littleEndian {
					r, b = b, r
				}
				img.Pix[d], img.Pix[d+1], img.Pix[d+2] = r, g, b
			}
			img.Pix[d+3] = 255
		}
	}
	return img, nil
}
