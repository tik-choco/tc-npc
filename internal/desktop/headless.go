//go:build !desktop

package desktop

import "errors"

var errNoDesktop = errors.New("this build has no desktop UI; rebuild with -tags desktop, or use `tc-npc serve`")

// RunApp blocks on the desktop UI event loop in a desktop build.
func RunApp(serverURL string, shutdown func()) error { return errNoDesktop }

// RunMascot opens the mascot window in a desktop build.
func RunMascot(serverURL string) error { return errNoDesktop }

// Available reports whether this build can open desktop windows.
const Available = false

// ErrNoDesktop is what RunApp/RunMascot return in this build.
var ErrNoDesktop = errNoDesktop
