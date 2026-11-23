//go:build desktop && (!windows || !production)

package desktop

import (
	"errors"
	"runtime"
)

var errUnsupported = errors.New("desktop UI is not supported on " + runtime.GOOS)

func init() {
	if runtime.GOOS == "windows" {
		errUnsupported = errors.New("desktop UI requires a Wails production build; rebuild with -tags desktop,production")
	}
}

// RunApp blocks on the desktop UI event loop in a supported build.
func RunApp(serverURL string, shutdown func()) error { return errUnsupported }

// RunMascot opens the mascot window in a supported build.
func RunMascot(serverURL string) error { return errUnsupported }

// Available reports whether this build can open desktop windows.
const Available = false

// ErrNoDesktop is what RunApp/RunMascot return in this build.
var ErrNoDesktop = errUnsupported
