package audio

import (
	"errors"
	"fmt"
	"math"
	"strconv"
	"strings"
)

// BGMMix describes an optional background track for MixAnnouncement.
type BGMMix struct {
	Bytes    []byte  // WAV or MP3 file contents.
	Volume   float32 // Resolved linear gain. Zero means silence.
	PlayFull bool    // Let the background track run to its end.
	EndTime  string  // Optional M:SS time at which to begin the fade.
}

const (
	duckTarget    = 0.5
	duckSmoothing = 0.05
	fadeSeconds   = 2
)

// MixAnnouncement scales a chime or speech clip, optionally mixes in a
// ducked background track, and returns a mono PCM16 WAV at the clip's rate.
// It runs synchronously. Call it in a goroutine when decoding and mixing a
// long track would block other speech work.
func MixAnnouncement(clip []byte, volume float32, bgm *BGMMix) ([]byte, error) {
	speech, rate, channels, err := DecodeAudio(clip)
	if err != nil {
		return nil, fmt.Errorf("decode announcement clip: %w", err)
	}
	if channels > 1 {
		speech = DownmixToMono(speech, channels)
	}
	if bgm == nil {
		output := make([]int16, len(speech))
		for i, s := range speech {
			output[i] = normalizedToPCM16(s * volume)
		}
		return EncodeWAVI16(output, rate)
	}

	background, bgmRate, bgmChannels, err := DecodeAudio(bgm.Bytes)
	if err != nil {
		return nil, fmt.Errorf("decode announcement BGM: %w", err)
	}
	if bgmChannels > 1 {
		background = DownmixToMono(background, bgmChannels)
	}
	// Put both clips on the speech sample clock for mixing.
	background = ResampleLinear(background, bgmRate, rate)

	fadeStart, cutoff := -1, math.MaxInt
	if seconds, ok := parseMMSS(bgm.EndTime); ok {
		samples := seconds * float32(rate)
		switch {
		case !(samples > 0): // Rust's unsigned cast maps negative values and NaN to zero.
			cutoff = 0
		case float64(samples) < float64(math.MaxInt):
			cutoff = int(samples)
		}
	}
	fadeSamples := max(float32(1), fadeSeconds*float32(rate))
	factor := float32(1)
	var output []int16
	for i := 0; ; i++ {
		speechDone, bgmDone := i >= len(speech), i >= len(background)
		forcedCutoff := i >= cutoff
		if speechDone && bgmDone && bgm.PlayFull && !forcedCutoff {
			break
		}
		if fadeStart < 0 && ((!bgm.PlayFull && speechDone) || forcedCutoff) {
			fadeStart = i
		}

		// Ease back toward the regular gain as soon as speech ends, even
		// if a forced fade is already running. The two gains multiply so
		// a fade during speech starts from the ducked level.
		target := float32(duckTarget)
		if speechDone {
			target = 1
		}
		factor += float32((target - factor) * duckSmoothing)
		gain := factor
		if fadeStart >= 0 {
			envelope := 1 - float32(i-fadeStart)/fadeSamples
			if envelope <= 0 {
				break
			}
			gain *= envelope
		}

		var voice, music float32
		if !speechDone {
			voice = float32(speech[i] * volume)
		}
		if !bgmDone {
			music = float32(background[i] * bgm.Volume * gain)
		}
		output = append(output, normalizedToPCM16(voice+music))
		// Cap pathological mixes at about ten minutes, including this sample.
		if uint64(i)+1 > uint64(rate)*600 {
			break
		}
	}
	return EncodeWAVI16(output, rate)
}

func parseMMSS(s string) (float32, bool) {
	minutes, seconds, ok := strings.Cut(s, ":")
	if !ok || strings.ContainsAny(s, "_xX") {
		return 0, false
	}
	m, minErr := strconv.ParseFloat(minutes, 32)
	sec, secErr := strconv.ParseFloat(seconds, 32)
	if minErr != nil && !errors.Is(minErr, strconv.ErrRange) ||
		secErr != nil && !errors.Is(secErr, strconv.ErrRange) {
		return 0, false
	}
	return float32(float32(m)*60) + float32(sec), true
}

func normalizedToPCM16(sample float32) int16 {
	if math.IsNaN(float64(sample)) {
		return 0
	}
	return int16(min(max(sample, -1), 1) * 32767)
}
