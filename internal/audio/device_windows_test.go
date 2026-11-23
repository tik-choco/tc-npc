package audio

import (
	"context"
	"errors"
	"math"
	"os"
	"strings"
	"testing"
	"time"
)

func requireAudioHardware(t *testing.T) {
	t.Helper()
	if os.Getenv("TC_NPC_AUDIO_HW_TEST") != "1" {
		t.Skip("set TC_NPC_AUDIO_HW_TEST=1 to use Windows audio devices")
	}
}

func TestHardwareListDevices(t *testing.T) {
	requireAudioHardware(t)
	for _, direction := range []struct {
		name string
		list func() ([]DeviceInfo, error)
	}{
		{"input", ListInputDevices},
		{"output", ListOutputDevices},
	} {
		t.Run(direction.name, func(t *testing.T) {
			devices, err := direction.list()
			if err != nil {
				t.Fatal(err)
			}
			t.Logf("devices: %+v", devices)
			seen := make(map[string]bool)
			defaults := 0
			for _, d := range devices {
				if strings.TrimSpace(d.Name) == "" || seen[d.Name] {
					t.Errorf("blank or duplicate device name %q", d.Name)
				}
				seen[d.Name] = true
				if d.IsDefault {
					defaults++
				}
			}
			if defaults > 1 {
				t.Errorf("found %d default devices", defaults)
			}
		})
	}
}

func TestHardwarePlaybackTone(t *testing.T) {
	requireAudioHardware(t)
	p, err := OpenPlayback("")
	if errors.Is(err, ErrNoDevice) {
		t.Skip(err)
	}
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := p.Close(); err != nil {
			t.Error(err)
		}
	})
	rate, channels := p.SampleRate(), p.Channels()
	frames := rate * 3 / 10
	tone := make([]float32, frames*channels)
	for frame := range frames {
		sample := float32(0.15 * math.Sin(2*math.Pi*440*float64(frame)/float64(rate)))
		for ch := range channels {
			tone[frame*channels+ch] = sample
		}
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	if err := p.Write(ctx, tone); err != nil {
		t.Fatal(err)
	}
	if err := p.Drain(ctx); err != nil {
		t.Fatal(err)
	}
	if got := p.Position(); got != uint64(frames) {
		t.Errorf("position = %d, played %d frames", got, frames)
	}
	t.Logf("played 0.3 s of 440 Hz at %d Hz, %d channels", rate, channels)
}

func TestHardwareCaptureRMS(t *testing.T) {
	requireAudioHardware(t)
	packets := make(chan []float32, 64)
	capture, err := OpenCapture("", func(samples []float32) {
		select {
		case packets <- append([]float32(nil), samples...):
		default:
		}
	})
	if errors.Is(err, ErrNoDevice) {
		t.Skip(err)
	}
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := capture.Close(); err != nil {
			t.Error(err)
		}
	})
	rate, channels := capture.SampleRate(), capture.Channels()
	target := (rate / 2) * channels
	samples := make([]float32, 0, target)
	timeout := time.NewTimer(3 * time.Second)
	defer timeout.Stop()
	for len(samples) < target {
		select {
		case packet := <-packets:
			samples = append(samples, packet[:min(len(packet), target-len(samples))]...)
		case <-timeout.C:
			t.Fatalf("captured %d/%d samples", len(samples), target)
		}
	}
	t.Logf("captured 0.5 s at %d Hz, %d channels, normalized RMS %.6f", rate, channels, RMSF32(samples))
}
