package speech

import (
	"math"
	"sync/atomic"
	"time"

	"github.com/tik-choco/tc-npc/internal/audio"
	"github.com/tik-choco/tc-npc/internal/config"
)

const (
	preRollMS          = 300
	startVoiceMS       = 120
	minVoicedMS        = 250
	minRMSFactor       = 0.25
	maxSegmentSecs     = 30
	defaultSilenceSecs = 1.5
)

// vadParams publishes all the live knobs together so each buffer sees a
// consistent threshold and barge-in factor, without waiting on a lock.
type vadParams struct {
	current atomic.Pointer[vadKnobs]
}

type vadKnobs struct {
	threshold     float32
	silenceMS     uint64
	bargeIn       bool
	bargeInFactor float32
}

func newVADParams(stt config.STTConfig) *vadParams {
	p := &vadParams{}
	p.set(stt)
	return p
}

func (p *vadParams) set(stt config.STTConfig) {
	threshold := stt.InputThreshold
	if !finite(threshold) {
		threshold = 0
	}
	secs := stt.SilenceDuration
	if !finite(secs) || secs <= 0 {
		secs = defaultSilenceSecs
	}
	// Multiply at float32 precision and saturate the rounded result like Rust.
	millis := math.Round(float64(float32(secs * 1000)))
	silenceMS := uint64(math.MaxUint64)
	if millis < float64(math.MaxUint64) {
		silenceMS = uint64(millis)
	}
	factor := stt.BargeInFactor
	if !finite(factor) {
		factor = 1
	}
	p.current.Store(&vadKnobs{max(threshold, 0), silenceMS, stt.BargeIn, max(factor, 1)})
}

func finite(v float32) bool {
	return v >= -math.MaxFloat32 && v <= math.MaxFloat32
}

func (p *vadKnobs) bargeInThreshold() float32 { return p.threshold * p.bargeInFactor }

type vadState struct {
	sampleRate      uint32
	params          *vadParams
	preRollLimit    int
	startVoiceLimit int

	preRoll          []int16
	startVoice       []int16
	pcm              []int16
	speechStarted    bool
	voicedSamples    int
	lastVoiceAt      time.Time
	segmentStartedAt time.Time
}

func newVADState(rate uint32, params *vadParams) *vadState {
	return &vadState{
		sampleRate: rate, params: params,
		preRollLimit: msToSamples(rate, preRollMS), startVoiceLimit: msToSamples(rate, startVoiceMS),
	}
}

func (v *vadState) process(chunk []int16, now time.Time) []int16 {
	knobs := v.params.current.Load()
	speaking := knobs.threshold <= 0 || audio.RMSI16(chunk) > knobs.threshold
	if !v.speechStarted {
		if speaking {
			v.startVoice = append(v.startVoice, chunk...)
			if len(v.startVoice) >= v.startVoiceLimit {
				v.pcm = append(v.pcm[:0], v.preRoll...)
				v.preRoll = v.preRoll[:0]
				v.pcm = append(v.pcm, v.startVoice...)
				v.voicedSamples = len(v.startVoice)
				v.startVoice = v.startVoice[:0]
				v.speechStarted = true
				v.lastVoiceAt, v.segmentStartedAt = now, now
			}
		} else {
			v.startVoice = v.startVoice[:0]
			v.preRoll = appendWithLimit(v.preRoll, chunk, v.preRollLimit)
		}
		return nil
	}

	if speaking {
		v.lastVoiceAt = now
		v.voicedSamples += len(chunk)
	}
	v.pcm = append(v.pcm, chunk...)

	elapsed := now.Sub(v.segmentStartedAt)
	// Duration::as_secs_f32 rounds the seconds and subsecond part separately.
	seconds := float32(elapsed/time.Second) + float32(elapsed%time.Second)/1e9
	if uint64(now.Sub(v.lastVoiceAt)/time.Millisecond) >= knobs.silenceMS || seconds >= maxSegmentSecs {
		return v.finishSegment()
	}
	return nil
}

func (v *vadState) flush() []int16 {
	if v.speechStarted && len(v.pcm) != 0 {
		return v.finishSegment()
	}
	return nil
}

// Reset drops partial speech when playback or the operator mutes the mic.
func (v *vadState) reset() {
	v.preRoll = v.preRoll[:0]
	v.startVoice = v.startVoice[:0]
	v.pcm = v.pcm[:0]
	v.speechStarted = false
	v.voicedSamples = 0
	v.lastVoiceAt, v.segmentStartedAt = time.Time{}, time.Time{}
}

func (v *vadState) finishSegment() []int16 {
	voicedMS := uint64(v.voicedSamples) * 1000 / uint64(max(v.sampleRate, 1))
	pcm := v.pcm
	v.pcm = nil
	v.reset()
	if len(pcm) == 0 || voicedMS < minVoicedMS {
		return nil
	}
	threshold := v.params.current.Load().threshold
	if threshold > 0 && audio.RMSI16(pcm) < threshold*minRMSFactor {
		return nil
	}
	return pcm
}

func msToSamples(rate uint32, ms uint64) int { return int(uint64(rate) * ms / 1000) }

func appendWithLimit(dst, data []int16, limit int) []int16 {
	if len(data) >= limit {
		return append(dst[:0], data[len(data)-limit:]...)
	}
	if excess := len(dst) + len(data) - limit; excess > 0 {
		dst = dst[:copy(dst, dst[excess:])]
	}
	return append(dst, data...)
}
