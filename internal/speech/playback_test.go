package speech

import (
	"context"
	"slices"
	"sync/atomic"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/audio"
	"github.com/tik-choco/tc-npc/internal/bus"
)

type fakePlayback struct {
	position atomic.Uint64
	writes   chan []float32
	flushed  chan struct{}
	drained  chan struct{}
}

func newFakePlayback() *fakePlayback {
	return &fakePlayback{writes: make(chan []float32, 8), flushed: make(chan struct{}, 8), drained: make(chan struct{}, 8)}
}

func (*fakePlayback) SampleRate() int { return 16000 }
func (*fakePlayback) Channels() int   { return 2 }
func (p *fakePlayback) Write(ctx context.Context, samples []float32) error {
	select {
	case p.writes <- slices.Clone(samples):
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (p *fakePlayback) Drain(ctx context.Context) error {
	select {
	case <-p.drained:
		return nil
	case <-ctx.Done():
		return ctx.Err()
	}
}

func (p *fakePlayback) Position() uint64 { return p.position.Load() }
func (p *fakePlayback) Flush() {
	p.position.Store(0)
	p.flushed <- struct{}{}
}
func (*fakePlayback) Close() error { return nil }

func await[T any](t *testing.T, c <-chan T) T {
	t.Helper()
	select {
	case value := <-c:
		return value
	case <-time.After(5 * time.Second):
		t.Fatal("timed out waiting for a channel value")
		var zero T
		return zero
	}
}

func awaitMessage(t *testing.T, sub *bus.Subscription, topic, kind string) bus.Message {
	t.Helper()
	ctx, cancel := context.WithTimeout(t.Context(), 5*time.Second)
	defer cancel()
	for {
		select {
		case msg := <-sub.C:
			if msg.Topic == topic && msg.Type == kind {
				return msg
			}
		case <-ctx.Done():
			t.Fatalf("timed out waiting for %s/%s", topic, kind)
			return bus.Message{}
		}
	}
}

func makeWAV(t *testing.T, sample int16, length, rate int) []byte {
	t.Helper()
	wav, err := audio.EncodeWAVI16(repeated(sample, length), rate)
	if err != nil {
		t.Fatal(err)
	}
	return wav
}

func TestPlaybackPublishesPositionLevelAndFlushesOnInterrupt(t *testing.T) {
	b := bus.New()
	sub := b.Subscribe()
	defer sub.Close()
	gate := &speakingGate{bus: b}
	out := newFakePlayback()
	// Position is cumulative across clips. The already consumed frames
	// must not be included in this clip's mouth animation.
	out.position.Store(8000)
	p := startPlayback(t.Context(), "spk", gate, func(string) (audio.Playback, error) { return out, nil })
	t.Cleanup(p.close)
	p.enqueue([][]byte{makeWAV(t, 4096, 8000, 8000)}, false)
	if samples := await(t, out.writes); len(samples) != 32000 || samples[0] != 0.125 || samples[1] != 0.125 {
		t.Fatal("playback did not resample and duplicate mono into stereo")
	}
	if msg := awaitMessage(t, sub, bus.TopicUI, UIMsgSpeaking); msg.Map()["active"] != true {
		t.Fatal("initial speaking transition was not active")
	}
	if len(sub.C) != 0 {
		t.Fatal("reported a level before any frame of this clip played")
	}
	out.position.Add(120)
	if msg := awaitMessage(t, sub, bus.TopicUI, UIMsgSpeakingLevel); msg.Map()["level"] != 0.75 {
		t.Errorf("speaking level = %s, want 0.75", msg.Payload)
	}
	p.interrupt(nil)
	await(t, out.flushed)
	if msg := awaitMessage(t, sub, bus.TopicUI, UIMsgSpeaking); msg.Map()["active"] != false {
		t.Fatal("interrupt did not publish inactive")
	}
	if msg := awaitMessage(t, sub, bus.TopicUI, UIMsgSpeakingLevel); msg.Map()["level"] != float64(0) {
		t.Errorf("final speaking level = %s, want zero", msg.Payload)
	}
}

func TestPlaybackLevelGainAndClamp(t *testing.T) {
	// The Rust gain and clamp now live in the audio package.
	if audio.PlaybackLevel(0) != 0 || audio.PlaybackLevel(0.12) <= 0.5 ||
		audio.PlaybackLevel(1) != 1 || audio.PlaybackLevel(5) != 1 {
		t.Fatal("playback level changed at silence, ordinary speech, or clipping")
	}
}
