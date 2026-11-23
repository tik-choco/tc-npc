package audio

import (
	"slices"
	"testing"
)

func TestUniqueNamedDevices(t *testing.T) {
	devices := []DeviceInfo{
		{Name: "  "}, {Name: "Mic"}, {Name: "Headset"}, {Name: "Mic", IsDefault: true},
	}
	want := []DeviceInfo{{Name: "Mic", IsDefault: true}, {Name: "Headset"}}
	if got := uniqueDeviceNames(devices); !slices.Equal(got, want) {
		t.Errorf("devices = %v, want %v", got, want)
	}
}

func TestDeviceNameSubstring(t *testing.T) {
	if !matchesDeviceName("Microphone (USB)", "usb") {
		t.Error("expected a case-insensitive substring match")
	}
	for _, needle := range []string{"", "speaker"} {
		if matchesDeviceName("Microphone (USB)", needle) {
			t.Errorf("unexpected match for %q", needle)
		}
	}
}

func TestPlaybackLevel(t *testing.T) {
	t.Run("silence maps to zero", func(t *testing.T) {
		if got := PlaybackLevel(0); got != 0 {
			t.Errorf("silence level = %g", got)
		}
	})
	t.Run("typical speech reads as mostly open", func(t *testing.T) {
		if got := PlaybackLevel(0.12); got <= 0.5 {
			t.Errorf("speech level = %g", got)
		}
	})
	t.Run("loud RMS clamps to one", func(t *testing.T) {
		for _, rms := range []float32{1, 5} {
			if got := PlaybackLevel(rms); got != 1 {
				t.Errorf("PlaybackLevel(%g) = %g", rms, got)
			}
		}
	})
}
