package speech

import (
	"math"
	"slices"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/config"
)

func loudChunk(n int) []int16 { return repeated(int16(20000), n) }

func repeated[T any](v T, n int) []T {
	out := make([]T, n)
	for i := range out {
		out[i] = v
	}
	return out
}

func testParams(threshold, silence float32) *vadParams {
	cfg := config.DefaultSTTConfig()
	cfg.InputThreshold, cfg.SilenceDuration = threshold, silence
	return newVADParams(cfg)
}

func TestVADShortBlipNeverStartsSegment(t *testing.T) {
	v := newVADState(16000, testParams(0.1, 1.5))
	now := time.Now()
	if v.process(loudChunk(100), now) != nil || v.process(make([]int16, 100), now) != nil || v.speechStarted {
		t.Fatal("short blip opened a segment")
	}
}

func TestVADSustainedSpeechThenSilence(t *testing.T) {
	v := newVADState(16000, testParams(0.1, 0.05))
	now := time.Now()
	for range 10 {
		now = now.Add(100 * time.Millisecond)
		if v.process(loudChunk(1600), now) != nil {
			t.Fatal("segment finished during speech")
		}
	}
	if got := v.process(make([]int16, 100), now.Add(60*time.Millisecond)); len(got) != 16100 {
		t.Errorf("finished segment has %d samples, want 16100", len(got))
	}
}

func TestVADThresholdUpdatesMidStream(t *testing.T) {
	knobs := testParams(0.1, 1.5)
	v := newVADState(16000, knobs)
	now := time.Now()
	for range 3 {
		v.process(loudChunk(1600), now)
	}
	if !v.speechStarted {
		t.Fatal("speech did not start at the initial threshold")
	}
	v.reset()
	stt := config.DefaultSTTConfig()
	stt.InputThreshold = 0.9
	knobs.set(stt)
	for range 3 {
		v.process(loudChunk(1600), now)
	}
	if v.speechStarted {
		t.Error("same audio started speech after increasing the threshold")
	}
}

func TestVADSilenceFallback(t *testing.T) {
	for _, bad := range []float32{0, -1, float32(math.NaN())} {
		if got := testParams(0.1, bad).current.Load().silenceMS; got != 1500 {
			t.Errorf("silence duration %v produced %d ms, want 1500", bad, got)
		}
	}
	if got := testParams(0.1, 0.4).current.Load().silenceMS; got != 400 {
		t.Errorf("silence duration %d ms, want 400", got)
	}
}

func TestVADBargeInFactorIsAtLeastOne(t *testing.T) {
	knobs := testParams(0.02, 1)
	if got := knobs.current.Load().bargeInThreshold(); math.Abs(float64(got-0.02*2.5)) >= 1e-6 {
		t.Errorf("default barge-in threshold %g, want %g", got, 0.02*2.5)
	}
	cfg := config.DefaultSTTConfig()
	cfg.InputThreshold, cfg.BargeInFactor = 0.02, 0.1
	knobs.set(cfg)
	if got := knobs.current.Load().bargeInThreshold(); math.Abs(float64(got-0.02)) >= 1e-6 {
		t.Errorf("barge-in threshold %g, want 0.02", got)
	}
}

func TestVADPreRollAndDurationBounds(t *testing.T) {
	v := newVADState(1000, testParams(0.1, 0.05))
	now := time.Now()
	v.process(repeated(int16(1), 250), now)
	v.process(repeated(int16(2), 100), now)
	v.process(loudChunk(120), now)
	v.process(loudChunk(130), now)
	if got := v.process(make([]int16, 10), now.Add(49*time.Millisecond)); got != nil {
		t.Fatal("silence closed the segment before 50 ms")
	}
	got := v.process(make([]int16, 10), now.Add(50*time.Millisecond))
	if len(got) != 570 || !slices.Equal(got[:200], repeated(int16(1), 200)) ||
		!slices.Equal(got[200:300], repeated(int16(2), 100)) {
		t.Fatalf("unexpected pre-roll or segment length %d", len(got))
	}

	v.process(loudChunk(249), now)
	if v.flush() != nil {
		t.Fatal("transcribed a segment under 250 ms voiced")
	}
	v.process(loudChunk(250), now)
	if got := v.process(loudChunk(1), now.Add(30*time.Second)); len(got) != 251 {
		t.Fatalf("30 s cap returned %d samples, want 251", len(got))
	}
	v.process(loudChunk(250), now)
	if v.process(make([]int16, 300000), now.Add(50*time.Millisecond)) != nil {
		t.Fatal("transcribed a segment below the minimum overall RMS")
	}
}
