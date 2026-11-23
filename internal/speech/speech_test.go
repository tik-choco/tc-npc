package speech

import (
	"context"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/audio"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/module"
)

type fakeCapture struct{}

func (fakeCapture) SampleRate() int { return 32000 }
func (fakeCapture) Channels() int   { return 2 }
func (fakeCapture) Close() error    { return nil }

func TestConfigRestartsOnlyAffectedAudioWorker(t *testing.T) {
	m := speechModule{
		openCapture:  func(string, func([]float32)) (audio.Capture, error) { return fakeCapture{}, nil },
		openPlayback: func(string) (audio.Playback, error) { return newFakePlayback(), nil },
	}
	cfg := config.DefaultConfig()
	cfg.TTS.Enabled, cfg.STT.Enabled = true, true
	cfg.Speech.InputDevice, cfg.Speech.OutputDevice = "mic", "spk"
	s := newSession(t.Context(), module.Ctx{Bus: bus.New(), Config: &cfg}, m)
	t.Cleanup(s.close)
	if playbackNeedsRestart(nil, cfg.Speech) || captureNeedsRestart(nil, cfg.Speech) {
		t.Fatal("stopped workers reported needing a restart")
	}
	s.applyConfig(&cfg)
	playback, capture := s.playback, s.capture
	update := func(next config.Config) {
		payload, err := json.Marshal(next)
		if err != nil {
			t.Fatal(err)
		}
		s.handleMessage(bus.Message{Topic: bus.TopicConfig, Type: bus.MsgConfigUpdated, Payload: payload})
	}
	cfg.STT.InputThreshold, cfg.TTS.Voice, cfg.Speech.OutputSampleRate = 0.25, "shimmer", 48000
	update(cfg)
	if s.playback != playback || s.capture != capture || s.vad.current.Load().threshold != 0.25 || s.cfg.TTS.Voice != "shimmer" {
		t.Fatal("live settings should update without restarting either audio worker")
	}
	cfg.Speech.OutputDevice = "headset"
	update(cfg)
	if s.playback == playback || s.capture != capture {
		t.Fatal("output device change did not restart only playback")
	}
	await(t, playback.done)
	playback = s.playback
	cfg.Speech.InputSampleRate = 48000
	update(cfg)
	if s.playback != playback || s.capture == capture {
		t.Fatal("input sample rate change did not restart only capture")
	}
	await(t, capture.done)
	capture = s.capture
	cfg.Speech.InputDevice = "headset mic"
	update(cfg)
	if s.playback != playback || s.capture == capture {
		t.Fatal("input device change did not restart only capture")
	}
	cfg.STT.Enabled, cfg.TTS.Enabled = false, false
	update(cfg)
	if s.playback != nil || s.capture != nil {
		t.Fatal("disabling STT and TTS left an audio worker attached")
	}
}

func TestSuspendTimerAndVoiceGateAreIndependent(t *testing.T) {
	s := testTTSSession(t, true)
	rx := s.bus.Subscribe()
	defer rx.Close()
	s.handleMessage(bus.Message{Topic: bus.TopicInterrupt, Type: bus.MsgSuspend})
	s.startReply("suspended!")
	s.tasks.Wait()
	if len(rx.C) != 0 || s.resume == nil {
		t.Fatal("suspend did not gate replies and arm a timeout")
	}
	s.setSuspended(time.Millisecond)
	await(t, s.resume)
	s.setSuspended(0)
	s.startReply("after expiry!")
	s.tasks.Wait()
	if msg := awaitMessage(t, rx, bus.TopicUI, UIMsgTTSLine); msg.Str("text") != "after expiry!" {
		t.Fatalf("unexpected line after expiry: %s", msg.Payload)
	}
	s.handleMessage(bus.Message{Topic: bus.TopicInterrupt, Type: bus.MsgVoiceStop})
	s.handleMessage(bus.Message{Topic: bus.TopicInterrupt, Type: bus.MsgResume})
	s.startReply("voice stopped!")
	s.tasks.Wait()
	if len(rx.C) != 0 || s.voice.Load()%2 != 0 {
		t.Fatal("resume reopened the operator's voice gate")
	}
	s.handleMessage(bus.Message{Topic: bus.TopicInterrupt, Type: bus.MsgVoiceStart})
	s.startReply("voice started!")
	s.tasks.Wait()
	if msg := awaitMessage(t, rx, bus.TopicUI, UIMsgTTSLine); msg.Str("text") != "voice started!" {
		t.Fatalf("unexpected line on voice start: %s", msg.Payload)
	}
}

func TestVoiceCascadeBargeInAndPriorityAudio(t *testing.T) {
	const sampleRate = 16000
	wav := makeWAV(t, 8192, 1600, sampleRate)
	secondStarted, secondCanceled := make(chan struct{}), make(chan struct{})
	requests := make(chan string, 8)
	sttAudio := make(chan []byte, 1)
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch r.URL.Path {
		case "/v1/audio/speech":
			var body struct {
				Input, Model, Voice string
			}
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil || body.Model != "test-model" || body.Voice != "alloy" {
				t.Errorf("unexpected TTS request: %+v, %v", body, err)
			}
			requests <- body.Input
			if body.Input == "Second?" {
				close(secondStarted)
				<-r.Context().Done()
				close(secondCanceled)
				return
			}
			_, _ = w.Write(wav)
		case "/v1/audio/transcriptions":
			file, _, err := r.FormFile("file")
			if err != nil {
				t.Error(err)
				w.WriteHeader(http.StatusBadRequest)
				return
			}
			defer file.Close()
			if model := r.FormValue("model"); model != "test-model" {
				t.Errorf("STT model = %q", model)
			}
			data, err := io.ReadAll(file)
			if err != nil {
				t.Error(err)
			}
			sttAudio <- data
			_, _ = io.WriteString(w, `{"text":"  hello again  "}`)
		default:
			t.Errorf("unexpected endpoint %s", r.URL.Path)
			w.WriteHeader(http.StatusNotFound)
		}
	}))
	t.Cleanup(server.Close)

	cfg := config.DefaultConfig()
	cfg.Providers = []config.ProviderConfig{{ID: "test", BaseURL: server.URL, Enabled: true}}
	cfg.DefaultRef = &config.ModelRef{ProviderID: "test", Model: "test-model"}
	cfg.TTS.Enabled, cfg.STT.Enabled = true, true
	cfg.STT.InputThreshold, cfg.STT.SilenceDuration = 0.1, 0.005
	b := bus.New()
	frames := b.Subscribe()
	defer frames.Close()
	soundDir := t.TempDir()
	if err := os.Mkdir(filepath.Join(soundDir, "sound"), 0700); err != nil {
		t.Fatal(err)
	}
	for name, data := range map[string][]byte{
		"chime.wav": makeWAV(t, 16384, 1600, sampleRate),
		"bgm.wav":   makeWAV(t, 4096, 3200, sampleRate),
	} {
		if err := os.WriteFile(filepath.Join(soundDir, "sound", name), data, 0600); err != nil {
			t.Fatal(err)
		}
	}
	mc := module.Ctx{Bus: b, Config: &cfg, DataDir: soundDir}
	m, err := New(mc)
	if err != nil || m.Name() != "npc-speech" {
		t.Fatalf("New = %v, %v", m, err)
	}
	out := newFakePlayback()
	mic := make(chan func([]float32), 1)
	fakeModule := speechModule{
		openCapture: func(_ string, callback func([]float32)) (audio.Capture, error) {
			mic <- callback
			return fakeCapture{}, nil
		},
		openPlayback: func(string) (audio.Playback, error) { return out, nil },
	}
	ctx, cancel := context.WithCancel(t.Context())
	finished := make(chan error, 1)
	go func() { finished <- fakeModule.Run(ctx, mc) }()
	t.Cleanup(func() {
		cancel()
		if err := await(t, finished); err != nil {
			t.Error(err)
		}
	})
	feed := await(t, mic)
	b.Publish(bus.TopicChat, bus.MsgChatResponse, map[string]string{"content": "First! Second? Third!"})
	if line := awaitMessage(t, frames, bus.TopicUI, UIMsgTTSLine); line.Str("text") != "First!" {
		t.Fatalf("first TTS line = %s", line.Payload)
	}
	await(t, out.writes)
	await(t, secondStarted)
	if line := awaitMessage(t, frames, bus.TopicUI, UIMsgTTSLine); line.Str("text") != "Second?" {
		t.Fatalf("second TTS line = %s", line.Payload)
	}
	if first, second := await(t, requests), await(t, requests); first != "First!" || second != "Second?" {
		t.Fatalf("first two synthesis requests = %q, %q", first, second)
	}
	// The flush can publish the off edge before the interrupt reaches the bus.
	stopped := b.Subscribe()
	defer stopped.Close()
	feed(repeated(float32(0.5), 32000*2*120/1000))
	if msg := awaitMessage(t, frames, bus.TopicInterrupt, bus.MsgInterrupt); msg.Str("source") != "barge_in" {
		t.Errorf("interrupt = %s", msg.Payload)
	}
	await(t, secondCanceled)
	await(t, out.flushed)
	if msg := awaitMessage(t, stopped, bus.TopicUI, UIMsgSpeaking); msg.Map()["active"] != false {
		t.Fatalf("speaking remained active after barge-in: %s", msg.Payload)
	}
	// The normal speech path opens again after the interrupted clip finishes.
	// A one-shot chime and its BGM announcement can still play after barge-in.
	utterance := make([]float32, 8000*2)
	for i := 0; i < len(utterance); i += 2 {
		utterance[i], utterance[i+1] = 0.75, 0.25
	}
	feed(utterance)
	time.Sleep(20 * time.Millisecond)
	feed(make([]float32, 320))
	samples, rate, channels, err := audio.DecodeWAV(await(t, sttAudio))
	if err != nil || rate != sampleRate || channels != 1 || len(samples) != 4080 || samples[0] != 16383.0/32768 {
		t.Fatalf("STT WAV = %d samples at %d Hz with %d channels, %v", len(samples), rate, channels, err)
	}
	if msg := awaitMessage(t, frames, bus.TopicSense, bus.MsgSpeech); msg.Str("content") != "hello again" {
		t.Fatalf("speech payload = %s", msg.Payload)
	}
	b.Publish(bus.TopicInterrupt, bus.MsgTTS, map[string]any{
		"chime_file": "chime.wav", "content": "Announcement! Unsplit?", "volume": 0.5,
		"bgm_file": "bgm.wav", "bgm_volume": 1, "bgm_play_full": true,
	})
	if input := await(t, requests); input != "Announcement! Unsplit?" {
		t.Fatalf("unexpected synthesis request %q", input)
	}
	chime := await(t, out.writes)
	if len(chime) != 3200 || chime[0] < 0.24 || chime[0] > 0.25 {
		t.Fatalf("scaled chime has %d samples", len(chime))
	}
	feed(repeated(float32(0.9), 32000*2/2))
	// The chime stays active after sustained loud mic audio, until drained.
	select {
	case <-out.flushed:
		t.Fatal("barge-in cut a priority chime")
	case <-time.After(50 * time.Millisecond):
	}
	out.drained <- struct{}{}
	announcement := await(t, out.writes)
	if len(announcement) != 6400 || announcement[0] <= 0.2 {
		t.Fatalf("announcement has %d samples, expected TTS mixed with the full BGM", len(announcement))
	}
	b.Publish(bus.TopicInterrupt, bus.MsgInterrupt, nil)
	await(t, out.flushed)
}
