// Package speech runs the microphone-to-STT and bus-to-TTS voice cascade.
package speech

import (
	"context"
	"log/slog"
	"math"
	"sync"
	"sync/atomic"
	"time"

	"github.com/tik-choco/tc-npc/internal/audio"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/module"
)

const (
	// UIMsgVolume reports the microphone level to the UI.
	UIMsgVolume = "volume"
	// UIMsgSpeaking reports when the host starts or stops playing a clip.
	UIMsgSpeaking = "speaking"
	// UIMsgSpeakingLevel reports the played clip's loudness for lip-sync.
	UIMsgSpeakingLevel = "speaking_level"
	// UIMsgTTSLine announces a sentence just before its synthesis request.
	UIMsgTTSLine = "tts_line"

	suspendTimeout = 600 * time.Second
)

// New creates the speech module even when both audio directions are disabled.
func New(_ module.Ctx) (module.Module, error) {
	return speechModule{openCapture: audio.OpenCapture, openPlayback: audio.OpenPlayback}, nil
}

type speechModule struct {
	openCapture  func(string, func([]float32)) (audio.Capture, error)
	openPlayback func(string) (audio.Playback, error)
}

func (speechModule) Name() string { return "npc-speech" }

type session struct {
	speechModule
	ctx      context.Context
	bus      *bus.Bus
	dataDir  string
	vad      *vadParams
	speaking speakingGate
	voice    atomic.Uint64 // Changes on each voice transition and is odd while listening.
	tasks    sync.WaitGroup

	// The bus loop owns the timer and device lifecycle. Queuing a clip and
	// cancelling a reply are serialized with each other and with config swaps.
	capture      *captureWorker
	suspendTimer *time.Timer
	resume       <-chan time.Time
	mu           sync.Mutex
	cfg          *config.Config
	character    *character.Character
	playback     *playbackWorker
	ttsCtx       context.Context
	cancelTTS    context.CancelFunc
	cancelReply  context.CancelFunc
}

func (m speechModule) Run(ctx context.Context, mc module.Ctx) error {
	ctx, cancel := context.WithCancel(ctx)
	sub := mc.Bus.Subscribe()
	defer sub.Close()
	s := newSession(ctx, mc, m)
	defer func() {
		cancel()
		s.close()
	}()
	s.applyConfig(mc.Config)
	var dropped uint64
	for ctx.Err() == nil {
		select {
		case <-ctx.Done():
		case <-s.resume:
			s.setSuspended(0)
			slog.Info("npc-speech: suspend timed out, auto-resuming")
		case msg := <-sub.C:
			if total := sub.Dropped(); total != dropped {
				slog.Warn("npc-speech: bus receiver lagged", "skipped", total-dropped)
				dropped = total
			}
			s.handleMessage(msg)
		}
	}
	return nil
}

func newSession(ctx context.Context, mc module.Ctx, m speechModule) *session {
	s := &session{
		speechModule: m, ctx: ctx, bus: mc.Bus, dataDir: mc.DataDir,
		vad: newVADParams(mc.Config.STT), speaking: speakingGate{bus: mc.Bus},
	}
	s.voice.Store(1)
	s.updateConfig(mc.Config)
	return s
}

func (s *session) close() {
	s.setSuspended(0)
	s.stopPlayback()
	if s.capture != nil {
		s.capture.close()
	}
	s.tasks.Wait()
}

func (s *session) handleMessage(msg bus.Message) {
	switch {
	case msg.Topic == bus.TopicConfig && msg.Type == bus.MsgConfigUpdated:
		var cfg config.Config
		if err := msg.Decode(&cfg); err != nil {
			slog.Warn("npc-speech: ignoring malformed config update", "error", err)
			return
		}
		s.updateConfig(&cfg)
		s.applyConfig(&cfg)
	case msg.Topic == bus.TopicChat && msg.Type == bus.MsgChatResponse:
		s.startReply(msg.Str("content"))
	case msg.Topic == bus.TopicInterrupt:
		switch msg.Type {
		case bus.MsgTTS:
			s.startAnnouncement(msg)
		case bus.MsgSuspend:
			s.setSuspended(suspendTimeout)
			s.stopSpeech()
		case bus.MsgResume:
			s.setSuspended(0)
		case bus.MsgInterrupt:
			// Barge-in has already cut the specific clip that triggered it.
			if msg.Str("source") != "barge_in" {
				s.stopSpeech()
			}
		case bus.MsgVoiceStart:
			s.setVoice(true)
		case bus.MsgVoiceStop:
			s.setVoice(false)
			s.stopSpeech()
		}
	}
}

// A single timer owned by the bus loop prevents an older suspend timeout
// from lifting a newer suspend.
func (s *session) setSuspended(duration time.Duration) {
	if duration == 0 {
		if s.suspendTimer != nil {
			s.suspendTimer.Stop()
		}
		s.resume = nil
		return
	}
	if s.suspendTimer == nil {
		s.suspendTimer = time.NewTimer(duration)
	} else {
		s.suspendTimer.Reset(duration)
	}
	s.resume = s.suspendTimer.C
}

func (s *session) setVoice(active bool) {
	if (s.voice.Load()%2 == 1) != active {
		s.voice.Add(1)
	}
}

func (s *session) updateConfig(cfg *config.Config) {
	// The bus loop is the only writer. Cache the voice override by active ID
	// so an unrelated save doesn't read the character file again.
	activeCharacter := s.character
	if s.cfg == nil || s.cfg.Character.ActiveID != cfg.Character.ActiveID {
		var err error
		activeCharacter, err = character.ActiveCharacter(s.dataDir, cfg.Character.ActiveID)
		if err != nil {
			slog.Warn("npc-speech: failed to resolve active character, using default voice", "error", err)
		}
	}
	s.vad.set(cfg.STT)
	s.mu.Lock()
	defer s.mu.Unlock()
	s.cfg, s.character = cfg, activeCharacter
	if cfg.TTS.Enabled && s.ttsCtx == nil {
		s.ttsCtx, s.cancelTTS = context.WithCancel(s.ctx)
	} else if !cfg.TTS.Enabled && s.ttsCtx != nil {
		s.cancelTTS()
		s.ttsCtx, s.cancelTTS = nil, nil
		s.stopSpeechLocked()
	}
}

func playbackNeedsRestart(p *playbackWorker, cfg config.SpeechConfig) bool {
	return p != nil && p.device != cfg.OutputDevice
}

func captureNeedsRestart(c *captureWorker, cfg config.SpeechConfig) bool {
	return c != nil && (c.speech.InputDevice != cfg.InputDevice || c.speech.InputSampleRate != cfg.InputSampleRate)
}

func (s *session) applyConfig(cfg *config.Config) {
	if !cfg.TTS.Enabled || playbackNeedsRestart(s.playback, cfg.Speech) {
		s.stopPlayback()
	}
	if cfg.TTS.Enabled && s.playback == nil {
		s.mu.Lock()
		s.playback = startPlayback(s.ctx, cfg.Speech.OutputDevice, &s.speaking, s.openPlayback)
		s.mu.Unlock()
	}
	if s.capture != nil && (!cfg.STT.Enabled || captureNeedsRestart(s.capture, cfg.Speech)) {
		s.capture.close()
		s.capture = nil
	}
	if cfg.STT.Enabled && s.capture == nil {
		s.capture = s.startCapture(cfg.Speech)
	}
}

func (s *session) stopPlayback() {
	s.mu.Lock()
	p := s.playback
	s.playback = nil
	s.mu.Unlock()
	if p != nil {
		p.close()
	}
}

func (s *session) stopSpeech() {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.stopSpeechLocked()
}

func (s *session) stopSpeechLocked() {
	s.cancelReplyLocked()
	if s.playback != nil {
		s.playback.interrupt(nil)
	}
}

func (s *session) cancelReplyLocked() {
	if s.cancelReply != nil {
		s.cancelReply()
		s.cancelReply = nil
	}
}

func (s *session) bargeIn(heard *playbackClip, voice uint64) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.voice.Load() != voice || s.playback == nil || !s.playback.interrupt(heard) {
		return
	}
	s.cancelReplyLocked()
	s.bus.Publish(bus.TopicInterrupt, bus.MsgInterrupt, map[string]string{"source": "barge_in"})
}

func publishLevel(b *bus.Bus, message string, level float64) {
	var number any = level
	if math.IsNaN(level) || math.IsInf(level, 0) {
		number = nil // serde emits null for a non-finite level.
	}
	b.Publish(bus.TopicUI, message, map[string]any{"level": number})
}
