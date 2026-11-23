// Package audio provides file codecs, mixing and device streams for speech.
package audio

import (
	"context"
	"errors"
	"strings"
)

var (
	// ErrUnsupported indicates that device audio is unavailable on this OS.
	ErrUnsupported = errors.New("audio devices are not supported on this platform")
	// ErrNoDevice indicates that there is no usable default audio endpoint.
	ErrNoDevice = errors.New("no default audio device available")
	// ErrFlushed is returned to a Write or Drain interrupted by Flush.
	ErrFlushed = errors.New("playback was flushed")
)

// DeviceInfo describes a selectable audio endpoint.
type DeviceInfo struct {
	Name      string
	IsDefault bool
}

// Capture delivers normalized interleaved samples at the endpoint rate and
// channel count. Close waits for callbacks to finish and reports stream errors.
type Capture interface {
	SampleRate() int
	Channels() int
	Close() error
}

// Playback accepts interleaved samples at the endpoint rate and channel count.
// Callers can use Write, Drain, Position, Flush and Close from other goroutines.
type Playback interface {
	SampleRate() int
	Channels() int

	// Write blocks until all samples have been queued. The slice may be
	// reused after Write returns. Cancelling an active Write discards
	// queued audio. For cancellation after Write returns, call Flush.
	Write(ctx context.Context, interleaved []float32) error
	// Drain waits for the queued audio to finish rendering. Cancelling
	// an active Drain discards queued audio.
	Drain(ctx context.Context) error
	// Position counts frames consumed by the audio engine, including
	// across Writes. It resets to zero when queued audio is discarded.
	// Use frame ranges of the original buffer to measure playback RMS.
	Position() uint64
	// Flush immediately drops the endpoint buffer and interrupts the
	// active Write or Drain. The stream stays open for the next clip.
	Flush()
	Close() error
}

// PlaybackLevel scales a playback RMS reading to a lip-sync level in [0, 1].
// Normalized speech RMS is usually too small to drive the full mouth range.
func PlaybackLevel(rms float32) float64 {
	return min(max(float64(rms)*6, 0), 1)
}

// Device selection uses the first case-insensitive substring match in endpoint order.
func matchesDeviceName(name, needle string) bool {
	return needle != "" && strings.Contains(strings.ToLower(name), needle)
}

func uniqueDeviceNames(devices []DeviceInfo) []DeviceInfo {
	out := make([]DeviceInfo, 0, len(devices))
	seen := make(map[string]int, len(devices))
	for _, device := range devices {
		if strings.TrimSpace(device.Name) == "" {
			continue
		}
		if i, ok := seen[device.Name]; ok {
			out[i].IsDefault = out[i].IsDefault || device.IsDefault
			continue
		}
		seen[device.Name] = len(out)
		out = append(out, device)
	}
	return out
}
