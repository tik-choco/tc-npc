package audio

import (
	"math"
	"testing"
)

func testWAV(t *testing.T, samples []int16, rate int) []byte {
	t.Helper()
	wav, err := EncodeWAVI16(samples, rate)
	if err != nil {
		t.Fatal(err)
	}
	return wav
}

func testConstantWAV(t *testing.T, sample int16, length, rate int) []byte {
	t.Helper()
	samples := make([]int16, length)
	for i := range samples {
		samples[i] = sample
	}
	return testWAV(t, samples, rate)
}

func testMix(t *testing.T, clip []byte, volume float32, bgm *BGMMix) ([]float32, int, int) {
	t.Helper()
	mixed, err := MixAnnouncement(clip, volume, bgm)
	if err != nil {
		t.Fatal(err)
	}
	samples, rate, channels, err := DecodeWAV(mixed)
	if err != nil {
		t.Fatal(err)
	}
	return samples, rate, channels
}

func TestMixWithoutBGMScalesClipByVolume(t *testing.T) {
	original := []int16{20000, -20000, 15000, -15000, 0}
	out, rate, channels := testMix(t, testWAV(t, original, 8000), 0.5, nil)
	if rate != 8000 || channels != 1 || len(out) != len(original) {
		t.Fatalf("mixed %d samples at %d Hz with %d channels", len(out), rate, channels)
	}
	for i, sample := range out {
		want := float32(original[i]) / 65536
		if math.Abs(float64(sample-want)) >= 0.001 {
			t.Errorf("sample %d = %g, want %g", i, sample, want)
		}
	}
}

func TestShortBGMSilentTail(t *testing.T) {
	const rate = 100
	bgm := &BGMMix{Bytes: testConstantWAV(t, 10000, 10, rate), Volume: 0.8}
	out, outRate, _ := testMix(t, testConstantWAV(t, 10000, 50, rate), 1, bgm)
	if outRate != rate || len(out) != 50+fadeSeconds*rate {
		t.Fatalf("mixed %d samples at %d Hz", len(out), outRate)
	}
	for i, sample := range out[50:] {
		if sample != 0 {
			t.Errorf("tail sample %d = %g, want silence", i, sample)
		}
	}
}

func TestPlayFullWithLongerBGMExtendsPastTTS(t *testing.T) {
	const rate = 100
	bgm := &BGMMix{Bytes: testConstantWAV(t, 8000, 100, rate), Volume: 1, PlayFull: true}
	out, _, _ := testMix(t, testConstantWAV(t, 5000, 20, rate), 1, bgm)
	if len(out) != 100 {
		t.Fatalf("mixed %d samples, want all 100 BGM samples", len(out))
	}
}

func TestPlayFullFalseFadesBGM(t *testing.T) {
	const rate = 100
	bgm := &BGMMix{Bytes: testConstantWAV(t, 10000, 300, rate), Volume: 1}
	out, _, _ := testMix(t, testConstantWAV(t, 5000, 30, rate), 1, bgm)
	if len(out) != 30+fadeSeconds*rate {
		t.Fatalf("mixed %d samples", len(out))
	}
	early, late := math.Abs(float64(out[35])), math.Abs(float64(out[len(out)-2]))
	if early <= 0.05 || late >= 0.02 || early <= late*3 {
		t.Errorf("fade envelope = %g early, %g late", early, late)
	}
}

func TestMixedOutputClamps(t *testing.T) {
	const rate = 100
	clip := testConstantWAV(t, 32000, 10, rate)
	bgm := &BGMMix{Bytes: clip, Volume: 1, PlayFull: true}
	out, _, _ := testMix(t, clip, 1, bgm)
	for i, sample := range out {
		if sample != 32767.0/32768 {
			t.Errorf("sample %d = %g, want maximum PCM16", i, sample)
		}
	}
}

func TestForcedBGMFadeDuringSpeech(t *testing.T) {
	const rate = 100
	bgm := &BGMMix{Bytes: testConstantWAV(t, 16384, 800, rate), Volume: 1, PlayFull: true, EndTime: "0:01"}
	out, _, _ := testMix(t, testConstantWAV(t, 0, 500, rate), 1, bgm)
	if len(out) != 300 {
		t.Fatalf("mixed %d samples, want cutoff plus fade (300)", len(out))
	}
	if out[0] < 0.48 || out[0] > 0.49 || out[100] < 0.25 || out[100] > 0.26 ||
		out[200] < 0.124 || out[200] > 0.126 {
		t.Errorf("initial, cutoff and mid-fade levels = %g, %g, %g", out[0], out[100], out[200])
	}
}

func TestParseMMSSParsesMinutesAndSeconds(t *testing.T) {
	for input, want := range map[string]float32{"1:30": 90, "0:05": 5, "2:00": 120, "0:1.5": 1.5} {
		if got, ok := parseMMSS(input); !ok || got != want {
			t.Errorf("parseMMSS(%q) = %g, %v, want %g", input, got, ok, want)
		}
	}
}

func TestParseMMSSRejectsEmptyAndGarbage(t *testing.T) {
	for _, input := range []string{"", "garbage", "1:2:3", "1:xx", ":"} {
		if got, ok := parseMMSS(input); ok {
			t.Errorf("parseMMSS(%q) = %g, want invalid", input, got)
		}
	}
}
