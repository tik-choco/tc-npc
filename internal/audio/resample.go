package audio

import (
	"math"
	"slices"
)

// DownmixToMono averages the channels of each interleaved frame. A partial
// final frame still divides by the channel count. Mono is returned as a copy.
func DownmixToMono(data []float32, channels int) []float32 {
	if channels <= 1 {
		return slices.Clone(data)
	}
	mono := make([]float32, 0, len(data)/channels+1)
	for frame := range slices.Chunk(data, channels) {
		var sum float32
		for _, sample := range frame {
			sum += sample
		}
		mono = append(mono, sum/float32(channels))
	}
	return mono
}

// ResampleLinear resamples a mono buffer with linear interpolation. Each call
// is independent, with no anti-aliasing filter or state across buffers.
func ResampleLinear(input []float32, srcRate, dstRate int) []float32 {
	if len(input) == 0 || srcRate <= 0 || dstRate <= 0 || srcRate == dstRate {
		return slices.Clone(input)
	}
	ratio := float64(srcRate) / float64(dstRate)
	out := make([]float32, int(math.Floor(float64(len(input))/ratio)))
	last := len(input) - 1
	for i := range out {
		pos := float64(float64(i) * ratio)
		index := int(pos)
		frac := float32(pos - float64(index))
		s0, s1 := input[min(index, last)], input[min(index+1, last)]
		out[i] = s0 + float32((s1-s0)*frac)
	}
	return out
}

// RMSF32 returns the RMS of normalized float audio, or zero for an empty buffer.
func RMSF32(samples []float32) float32 {
	if len(samples) == 0 {
		return 0
	}
	var sum float64
	for _, sample := range samples {
		s := float64(sample)
		sum += float64(s * s)
	}
	return float32(math.Sqrt(sum / float64(len(samples))))
}

// RMSI16 returns the RMS of signed PCM16 audio normalized by 32768.
func RMSI16(samples []int16) float32 {
	if len(samples) == 0 {
		return 0
	}
	var sum float64
	for _, sample := range samples {
		s := float64(sample) / 32768
		sum += float64(s * s)
	}
	return float32(math.Sqrt(sum / float64(len(samples))))
}
