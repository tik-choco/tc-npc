package audio

import (
	"context"
	"errors"
	"sync/atomic"
	"testing"
	"time"
)

type testRenderDevice struct {
	queued  atomic.Int64
	written chan struct{}
}

func (d *testRenderDevice) padding() (int, error) { return int(d.queued.Load()), nil }
func (d *testRenderDevice) start() error          { return nil }
func (d *testRenderDevice) stop() error           { return nil }
func (d *testRenderDevice) reset() error {
	d.queued.Store(0)
	return nil
}

func (d *testRenderDevice) queue(stereo []float32) error {
	d.queued.Add(int64(len(stereo) / 2))
	select {
	case d.written <- struct{}{}:
	default:
	}
	return nil
}

func openTestPlayback(t *testing.T) (*playbackStream, *testRenderDevice) {
	t.Helper()
	device := &testRenderDevice{written: make(chan struct{}, 1)}
	p := newPlaybackStream()
	if err := p.open(func(ready func(int, int)) error {
		ready(48000, 2)
		return p.run(device, 2, time.Millisecond, 0)
	}); err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := p.Close(); err != nil {
			t.Error(err)
		}
	})
	return p, device
}

func TestPlaybackWriteAndDrainTrackDifferentStages(t *testing.T) {
	p, device := openTestPlayback(t)
	ctx, cancel := context.WithTimeout(context.Background(), 2*time.Second)
	defer cancel()
	if err := p.Write(ctx, []float32{0.25, 0.25, 0.5, 0.5}); err != nil {
		t.Fatal(err)
	}
	if p.Position() != 0 || device.queued.Load() != 2 {
		t.Fatalf("after Write: consumed %d and queued %d frames", p.Position(), device.queued.Load())
	}
	result := make(chan error, 1)
	go func() { result <- p.Drain(ctx) }()
	select {
	case err := <-result:
		t.Fatalf("Drain completed before consumption: %v", err)
	case <-time.After(10 * time.Millisecond):
	}
	device.queued.Store(0)
	if err := <-result; err != nil {
		t.Fatal(err)
	}
	if got := p.Position(); got != 2 {
		t.Errorf("after Drain: consumed %d frames, want 2", got)
	}
}

func TestPlaybackInterruptedWriteDiscardsAudio(t *testing.T) {
	for _, tc := range []struct {
		name string
		want error
	}{
		{"flush", ErrFlushed},
		{"cancel", context.Canceled},
	} {
		t.Run(tc.name, func(t *testing.T) {
			p, device := openTestPlayback(t)
			ctx, deadline := context.WithTimeout(context.Background(), 2*time.Second)
			defer deadline()
			writeCtx, cancelWrite := context.WithCancel(ctx)
			defer cancelWrite()
			result := make(chan error, 1)
			go func() { result <- p.Write(writeCtx, make([]float32, 8)) }()
			select {
			case <-device.written:
			case <-ctx.Done():
				t.Fatal("timed out waiting for the first write")
			}
			if tc.want == ErrFlushed {
				p.Flush()
			} else {
				cancelWrite()
			}
			if err := <-result; !errors.Is(err, tc.want) {
				t.Fatalf("Write returned %v, want %v", err, tc.want)
			}
			if p.Position() != 0 || device.queued.Load() != 0 {
				t.Fatalf("after interruption: consumed %d and queued %d frames", p.Position(), device.queued.Load())
			}
			if err := p.Write(ctx, []float32{0.5, 0.5}); err != nil {
				t.Fatalf("write after interruption: %v", err)
			}
		})
	}
}
