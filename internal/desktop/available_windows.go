//go:build desktop && production

package desktop

// Available reports whether this build can open desktop windows.
const Available = true

// ErrNoDesktop is nil in a desktop build.
var ErrNoDesktop error
