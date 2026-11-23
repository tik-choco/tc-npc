//go:build !windows

package audio

// ListInputDevices returns ErrUnsupported on this platform.
func ListInputDevices() ([]DeviceInfo, error) { return nil, ErrUnsupported }

// ListOutputDevices returns ErrUnsupported on this platform.
func ListOutputDevices() ([]DeviceInfo, error) { return nil, ErrUnsupported }

// OpenCapture returns ErrUnsupported on this platform.
func OpenCapture(deviceName string, onData func(interleaved []float32)) (Capture, error) {
	return nil, ErrUnsupported
}

// OpenPlayback returns ErrUnsupported on this platform.
func OpenPlayback(deviceName string) (Playback, error) { return nil, ErrUnsupported }
