package audio

import (
	"math"
	"slices"
	"testing"
)

func TestResampleNoopWhenRatesMatch(t *testing.T) {
	input := []float32{0.1, 0.2, 0.3}
	got := ResampleLinear(input, 16000, 16000)
	if !slices.Equal(got, input) || &got[0] == &input[0] {
		t.Fatalf("wanted a copy of %v, got %v", input, got)
	}
}

func TestResampleDownsamplesToExpectedLength(t *testing.T) {
	if got := ResampleLinear(make([]float32, 4800), 48000, 16000); len(got) != 1600 {
		t.Errorf("resampled length = %d, want 1600", len(got))
	}
}

func TestDownmixStereoAveragesChannels(t *testing.T) {
	got := DownmixToMono([]float32{1, -1, 0.5, 0.5}, 2)
	if want := []float32{0, 0.5}; !slices.Equal(got, want) {
		t.Errorf("DownmixToMono = %v, want %v", got, want)
	}
}

func TestResampleInterpolatesAndRepeatsLastSample(t *testing.T) {
	got := ResampleLinear([]float32{0, 1}, 1, 2)
	if want := []float32{0, 0.5, 1, 1}; !slices.Equal(got, want) {
		t.Errorf("ResampleLinear = %v, want %v", got, want)
	}
}

func TestRMS(t *testing.T) {
	if RMSF32(nil) != 0 || RMSI16(nil) != 0 {
		t.Fatal("RMS of an empty buffer must be zero")
	}
	want := float32(math.Sqrt(0.625))
	gotF32 := RMSF32([]float32{-1, 0.5})
	gotI16 := RMSI16([]int16{-32768, 16384})
	if gotF32 != want || gotI16 != want {
		t.Errorf("RMSF32 = %g, RMSI16 = %g, want %g", gotF32, gotI16, want)
	}
}
