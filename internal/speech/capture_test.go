package speech

import (
	"math"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/module"
)

const bargeInLimit = 1920 // 120 ms at 16 kHz.

func TestBargeInNeedsSustainedAudio(t *testing.T) {
	d := newBargeInDetector(16000)
	if d.observe(loudChunk(bargeInLimit/2), 0.1) || d.observe(make([]int16, 160), 0.1) ||
		d.observe(loudChunk(bargeInLimit/2), 0.1) {
		t.Fatal("barge-in fired for two short bursts separated by silence")
	}
}

func TestBargeInFiresOnceThenRearms(t *testing.T) {
	d := newBargeInDetector(16000)
	if d.observe(loudChunk(bargeInLimit/2), 0.1) || !d.observe(loudChunk(bargeInLimit/2), 0.1) {
		t.Fatal("barge-in must fire on the second consecutive half-window")
	}
	if d.observe(loudChunk(bargeInLimit), 0.1) {
		t.Fatal("barge-in fired twice for the same clip")
	}
	d.rearm()
	if !d.observe(loudChunk(bargeInLimit), 0.1) {
		t.Error("barge-in did not rearm for the next clip")
	}
}

func TestBargeInIgnoresBelowThreshold(t *testing.T) {
	d := newBargeInDetector(16000)
	if d.observe(loudChunk(bargeInLimit*4), 0.9) {
		t.Error("barge-in fired below threshold")
	}
}

func TestCaptureMeterAndEchoGate(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.STT.BargeIn = false
	b := bus.New()
	s := newSession(t.Context(), module.Ctx{Bus: b, Config: &cfg}, speechModule{})
	rx := b.Subscribe()
	defer rx.Close()
	c := captureProcessor{
		vad: newVADState(16000, s.vad), bargeIn: newBargeInDetector(16000),
		deviceRate: 16000, targetRate: 16000, channels: 1,
	}
	packet := capturePacket{samples: repeated(float32(0.15), bargeInLimit), at: time.Now(), voice: s.voice.Load()}
	c.process(s, packet)
	msg := awaitMessage(t, rx, bus.TopicUI, UIMsgVolume)
	if level, _ := msg.Map()["level"].(float64); math.Abs(level-0.6) > 1e-6 || !c.vad.speechStarted {
		t.Fatalf("listening packet gave volume %s and speechStarted=%v", msg.Payload, c.vad.speechStarted)
	}
	clip := &playbackClip{}
	s.speaking.set(clip)
	packet.playing = clip
	packet.at = packet.at.Add(time.Millisecond)
	c.process(s, packet)
	if c.vad.speechStarted || c.bargeIn.fired {
		t.Fatal("playback audio reached the VAD or triggered disabled barge-in")
	}
	s.speaking.set(nil)
	c.process(s, packet)
	if c.vad.speechStarted {
		t.Fatal("a delayed playback packet reached the VAD after the clip ended")
	}
	s.setVoice(false)
	packet.playing = nil
	packet.voice = s.voice.Load()
	packet.at = packet.at.Add(volumePublishInterval)
	c.process(s, packet)
	if msg := awaitMessage(t, rx, bus.TopicUI, UIMsgVolume); msg.Map()["level"] != float64(0) || c.vad.speechStarted {
		t.Fatalf("muted packet gave volume %s and speechStarted=%v", msg.Payload, c.vad.speechStarted)
	}
}
