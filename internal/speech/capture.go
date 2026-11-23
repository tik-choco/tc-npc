package speech

import (
	"context"
	"log/slog"
	"math"
	"slices"
	"strings"
	"time"

	"github.com/tik-choco/tc-npc/internal/audio"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/osc"
)

const (
	volumePublishInterval = 100 * time.Millisecond
	volumeUIGain          = 4
)

type bargeInDetector struct {
	loudSamples int
	limit       int
	fired       bool
}

func newBargeInDetector(rate uint32) bargeInDetector {
	return bargeInDetector{limit: msToSamples(rate, startVoiceMS)}
}

// Sustain filters out a cough or transient spike, and a clip can fire only
// once while the speaker winds down.
func (d *bargeInDetector) observe(chunk []int16, threshold float32) bool {
	if d.fired {
		return false
	}
	if !(threshold <= 0 || audio.RMSI16(chunk) > threshold) {
		d.loudSamples = 0
		return false
	}
	d.loudSamples += len(chunk)
	d.fired = d.loudSamples >= d.limit
	return d.fired
}

func (d *bargeInDetector) rearm() { d.loudSamples, d.fired = 0, false }

type captureWorker struct {
	speech config.SpeechConfig
	cancel context.CancelFunc
	done   chan struct{}
}

func (s *session) startCapture(cfg config.SpeechConfig) *captureWorker {
	ctx, cancel := context.WithCancel(s.ctx)
	c := &captureWorker{speech: cfg, cancel: cancel, done: make(chan struct{})}
	go func() {
		defer close(c.done)
		s.captureAudio(ctx, cfg)
	}()
	return c
}

func (c *captureWorker) close() {
	c.cancel()
	<-c.done
}

type capturePacket struct {
	samples []float32
	at      time.Time
	voice   uint64
	playing *playbackClip
}

func (s *session) captureAudio(ctx context.Context, cfg config.SpeechConfig) {
	// OpenCapture may deliver a packet before returning the endpoint format.
	// Keep the callback short and copy only when the worker has room.
	packets := make(chan capturePacket, 32)
	stream, err := s.openCapture(cfg.InputDevice, func(samples []float32) {
		if ctx.Err() != nil || len(packets) == cap(packets) {
			return
		}
		packet := capturePacket{
			at: time.Now(), voice: s.voice.Load(), playing: s.speaking.active.Load(), samples: slices.Clone(samples),
		}
		select {
		case packets <- packet:
		default:
		}
	})
	if err != nil {
		slog.Error("npc-speech: could not open capture device", "error", err)
		return
	}
	slog.Info("npc-speech: capturing mic input", "device", cfg.InputDevice,
		"rate", stream.SampleRate(), "channels", stream.Channels())
	rate := max(cfg.InputSampleRate, 1000)
	c := captureProcessor{
		vad: newVADState(rate, s.vad), bargeIn: newBargeInDetector(rate),
		deviceRate: stream.SampleRate(), channels: stream.Channels(), targetRate: int(rate),
	}
	for ctx.Err() == nil {
		select {
		case <-ctx.Done():
		case packet := <-packets:
			c.process(s, packet)
		}
	}
	if err := stream.Close(); err != nil {
		slog.Error("npc-speech: could not close capture device", "error", err)
	}
	// A device change or STT toggle still delivers any finished utterance.
	if s.voice.Load() == c.voice {
		c.sendSegment(s, c.vad.flush())
	}
}

type captureProcessor struct {
	vad                    *vadState
	bargeIn                bargeInDetector
	deviceRate, targetRate int
	channels               int
	lastVolume             time.Time
	voice                  uint64
	measuredClip           *playbackClip
}

func (c *captureProcessor) process(s *session, packet capturePacket) {
	samples := packet.samples
	if c.channels > 1 {
		samples = audio.DownmixToMono(samples, c.channels)
	}
	if c.deviceRate != c.targetRate {
		samples = audio.ResampleLinear(samples, c.deviceRate, c.targetRate)
	}
	if len(samples) == 0 {
		return
	}
	voice := s.voice.Load()
	listening := voice%2 == 1 && voice == packet.voice
	if packet.at.Sub(c.lastVolume) >= volumePublishInterval {
		c.lastVolume = packet.at
		level := float64(0)
		if listening {
			level = min(max(float64(audio.RMSF32(samples))*volumeUIGain, 0), 1)
		}
		publishLevel(s.bus, UIMsgVolume, level)
	}
	// The stream stays open when the gate is off. A generation change also
	// clears partial speech if stop and start arrived between two packets.
	if !listening || voice != c.voice {
		c.voice = voice
		c.vad.reset()
		c.bargeIn.rearm()
		c.measuredClip = nil
		if !listening {
			return
		}
	}
	playing := s.speaking.active.Load()
	if packet.playing != nil || playing != nil {
		c.vad.reset()
		if c.measuredClip != packet.playing {
			c.bargeIn.rearm()
			c.measuredClip = packet.playing
		}
		knobs := s.vad.current.Load()
		if playing != nil && playing == packet.playing && !playing.priority && knobs.bargeIn &&
			c.bargeIn.observe(pcm16(samples), knobs.bargeInThreshold()) {
			s.bargeIn(playing, voice)
		}
		return
	}
	c.bargeIn.rearm()
	c.measuredClip = nil
	c.sendSegment(s, c.vad.process(pcm16(samples), packet.at))
}

func pcm16(samples []float32) []int16 {
	pcm := make([]int16, len(samples))
	for i, sample := range samples {
		if !math.IsNaN(float64(sample)) {
			pcm[i] = int16(min(max(sample, -1), 1) * 32767)
		}
	}
	return pcm
}

func (c *captureProcessor) sendSegment(s *session, pcm []int16) {
	if len(pcm) == 0 || s.ctx.Err() != nil {
		return
	}
	wav, err := audio.EncodeWAVI16(pcm, c.targetRate)
	if err != nil {
		slog.Error("npc-speech: failed to encode wav segment", "error", err)
		return
	}
	s.tasks.Go(func() { s.transcribe(wav) })
}

func (s *session) transcribe(wav []byte) {
	s.mu.Lock()
	cfg := s.cfg
	s.mu.Unlock()
	resolved, err := cfg.Resolve(config.TaskSTT)
	if err != nil {
		slog.Error("npc-speech: stt model is unavailable", "error", err)
		return
	}
	text, err := llm.New(resolved.BaseURL, resolved.APIKey).Transcribe(s.ctx, resolved.Model, wav)
	if err != nil {
		if s.ctx.Err() == nil {
			slog.Error("npc-speech: stt transcription failed", "error", err)
		}
		return
	}
	if text = strings.TrimSpace(text); text == "" || s.ctx.Err() != nil {
		return
	}
	s.bus.Publish(bus.TopicSense, bus.MsgSpeech, bus.SpeechPayload(text, "", "", false))
	if cfg.VRC.Chatbox {
		osc.SendChatbox(cfg.VRC.OSCAddress, text)
	}
}
