package speech

import (
	"context"
	"log/slog"
	"os"
	"strings"
	"unicode/utf8"

	"github.com/tik-choco/tc-npc/internal/audio"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/osc"
	"github.com/tik-choco/tc-npc/internal/sound"
)

func (s *session) startReply(content string) {
	if content == "" || s.voice.Load()%2 == 0 || s.resume != nil {
		return
	}
	s.mu.Lock()
	if s.ttsCtx == nil {
		s.mu.Unlock()
		return
	}
	// A newer reply supersedes the remaining synthesis for an older one.
	s.cancelReplyLocked()
	ctx, cancel := context.WithCancel(s.ttsCtx)
	s.cancelReply = cancel
	cfg, active := s.cfg, s.character
	s.mu.Unlock()
	s.tasks.Go(func() {
		defer cancel()
		s.speakReply(ctx, cfg, active, content)
	})
}

func (s *session) speakReply(ctx context.Context, cfg *config.Config, active *character.Character, content string) {
	if !cfg.TTS.Enabled {
		return
	}
	for _, sentence := range splitSentences(content) {
		text := truncateForTTS(sentence, cfg.TTS.MaxLen)
		s.mu.Lock()
		if ctx.Err() != nil || !s.cfg.TTS.Enabled {
			s.mu.Unlock()
			return
		}
		// The UI sees the line before spending time on the synthesis request.
		s.bus.Publish(bus.TopicUI, UIMsgTTSLine, map[string]string{"text": text})
		s.mu.Unlock()

		wav, ok := synthesizeTTS(ctx, cfg, text, active)
		if !ok {
			continue
		}
		if !s.queueTTS(ctx, [][]byte{wav}, false) {
			return
		}
		if cfg.VRC.Chatbox {
			osc.SendChatbox(cfg.VRC.OSCAddress, text)
		}
	}
}

// Checking cancellation while holding the same lock as stopSpeech prevents
// a sentence that just finished synthesizing from racing back onto the queue.
func (s *session) queueTTS(ctx context.Context, items [][]byte, priority bool) bool {
	s.mu.Lock()
	defer s.mu.Unlock()
	if ctx.Err() != nil || !s.cfg.TTS.Enabled {
		return false
	}
	if s.playback != nil {
		s.playback.enqueue(items, priority)
	}
	return true
}

type announcement struct {
	content, chimeFile, bgmFile, bgmEndTime string
	volume, bgmVolume                       float32
	bgmPlayFull                             bool
}

func (s *session) startAnnouncement(msg bus.Message) {
	s.mu.Lock()
	ctx, cfg, active := s.ttsCtx, s.cfg, s.character
	s.mu.Unlock()
	if ctx == nil {
		return
	}
	payload := msg.Map()
	content, _ := payload["content"].(string)
	chime, _ := payload["chime_file"].(string)
	if content == "" && chime == "" {
		return
	}
	volume, ok := payload["volume"].(float64)
	if !ok {
		volume = 1
	}
	bgm, _ := payload["bgm_file"].(string)
	bgmVolume, _ := payload["bgm_volume"].(float64)
	playFull, _ := payload["bgm_play_full"].(bool)
	endTime, _ := payload["bgm_end_time"].(string)
	a := announcement{
		content: content, chimeFile: chime, volume: float32(volume),
		bgmFile: bgm, bgmVolume: float32(bgmVolume), bgmPlayFull: playFull, bgmEndTime: endTime,
	}
	s.tasks.Go(func() { s.playAnnouncement(ctx, cfg, active, a) })
}

func (s *session) playAnnouncement(ctx context.Context, cfg *config.Config, active *character.Character, a announcement) {
	if !cfg.TTS.Enabled || ctx.Err() != nil {
		return
	}
	var items [][]byte
	if a.chimeFile != "" {
		path := sound.ResolveSoundPath(s.dataDir, a.chimeFile)
		clip, err := os.ReadFile(path)
		if err != nil {
			slog.Warn("npc-speech: chime_file not found, skipping", "file", a.chimeFile, "path", path, "error", err)
		} else if mixed, err := audio.MixAnnouncement(clip, a.volume, nil); err != nil {
			slog.Warn("npc-speech: failed to decode chime_file, skipping", "file", a.chimeFile, "path", path, "error", err)
		} else {
			items = append(items, mixed)
		}
	}
	if a.content != "" && ctx.Err() == nil {
		// Announcements are one deliberate utterance, with an optional chime
		// ahead of it and optional BGM mixed into the utterance itself.
		text := truncateForTTS(a.content, cfg.TTS.MaxLen)
		if wav, ok := synthesizeTTS(ctx, cfg, text, active); ok && ctx.Err() == nil {
			if cfg.VRC.Chatbox {
				osc.SendChatbox(cfg.VRC.OSCAddress, text)
			}
			bgm := s.readBGM(a)
			if mixed, err := audio.MixAnnouncement(wav, a.volume, bgm); err != nil {
				slog.Warn("npc-speech: failed to mix tts announcement, skipping", "error", err)
			} else {
				items = append(items, mixed)
			}
		}
	}
	if len(items) > 0 {
		s.queueTTS(ctx, items, true)
	}
}

func (s *session) readBGM(a announcement) *audio.BGMMix {
	if a.bgmFile == "" {
		return nil
	}
	path := sound.ResolveSoundPath(s.dataDir, a.bgmFile)
	bgm, err := os.ReadFile(path)
	if err != nil {
		slog.Warn("npc-speech: bgm_file not found, playing tts without bgm", "file", a.bgmFile, "path", path, "error", err)
		return nil
	}
	return &audio.BGMMix{Bytes: bgm, Volume: a.bgmVolume, PlayFull: a.bgmPlayFull, EndTime: a.bgmEndTime}
}

func synthesizeTTS(ctx context.Context, cfg *config.Config, text string, active *character.Character) ([]byte, bool) {
	if !cfg.TTS.Enabled || ctx.Err() != nil {
		return nil, false
	}
	resolved, err := cfg.Resolve(config.TaskTTS)
	if err != nil {
		slog.Error("npc-speech: tts model is unavailable", "error", err)
		return nil, false
	}
	voice, model := resolveVoice(active, cfg.TTS.Voice, resolved.Model)
	wav, err := llm.New(resolved.BaseURL, resolved.APIKey).Speak(ctx, model, voice, text, cfg.TTS.Speed)
	if err != nil {
		if ctx.Err() == nil {
			slog.Error("npc-speech: tts synthesis failed", "error", err)
		}
		return nil, false
	}
	return wav, true
}

func resolveVoice(active *character.Character, voice, model string) (string, string) {
	if active != nil {
		if active.VoiceName != nil && *active.VoiceName != "" {
			voice = *active.VoiceName
		}
		if active.VoiceModel != nil && *active.VoiceModel != "" {
			model = *active.VoiceModel
		}
	}
	return voice, model
}

func truncateForTTS(text string, limit uint32) string {
	if limit > 0 {
		for i := range text {
			if limit == 0 {
				return text[:i] + "..."
			}
			limit--
		}
	}
	return text
}

// Each terminator ends its own fragment. A period is common inside words
// and numbers in the reference splitter, so only the full-width period splits.
func splitSentences(text string) []string {
	var sentences []string
	start := 0
	for i, ch := range text {
		switch ch {
		case '。', '！', '？', '!', '?', '\n':
			end := i + utf8.RuneLen(ch)
			if sentence := strings.TrimSpace(text[start:end]); sentence != "" {
				sentences = append(sentences, sentence)
			}
			start = end
		}
	}
	if sentence := strings.TrimSpace(text[start:]); sentence != "" {
		sentences = append(sentences, sentence)
	}
	return sentences
}
