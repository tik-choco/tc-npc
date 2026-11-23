// Package vision periodically describes a screen or window with a vision model.
package vision

import (
	"context"
	"log/slog"
	"math"
	"sync"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/module"
)

type visionModule struct{}

// New constructs the npc-vision module.
func New(_ module.Ctx) (module.Module, error) {
	return visionModule{}, nil
}

func (visionModule) Name() string { return "npc-vision" }

func (visionModule) Run(ctx context.Context, mc module.Ctx) error {
	cfg := mc.Config.Vision
	if !cfg.Enabled || ctx.Err() != nil {
		return nil
	}
	resolved, err := mc.Config.Resolve(config.TaskVision)
	if err != nil {
		return err
	}
	client := llm.New(resolved.BaseURL, resolved.APIKey).WithReasoningEffort(resolved.ReasoningEffort)

	// Keep the language directive last so it reads as the final output instruction.
	prompt := cfg.SystemPrompt
	if cfg.PersonDetection {
		prompt += "\n\n画面に人物が写っている場合は observe_person ツールで記録してください。"
	}
	prompt = config.WithLanguageInstruction(prompt, mc.Config.Language)

	var memory speechMemory
	sub := mc.Bus.Subscribe()
	defer sub.Close()
	go memory.track(ctx, sub)

	seconds := max(0.1, cfg.IntervalSeconds)
	if math.IsNaN(float64(seconds)) {
		seconds = 0.1
	}
	nanos := math.Round(float64(seconds) * float64(time.Second))
	interval := time.Duration(math.MaxInt64)
	if nanos < float64(interval) {
		interval = time.Duration(nanos)
	}
	// Wait from the end of each cycle so capture and analysis never overlap.
	for ctx.Err() == nil {
		runCycle(ctx, client, resolved.Model, cfg, prompt, mc, &memory)
		timer := time.NewTimer(interval)
		select {
		case <-ctx.Done():
			timer.Stop()
			return nil
		case <-timer.C:
		}
	}
	return nil
}

// The latest speech content supplies short-term context for the next VLM call.
type speechMemory struct {
	mu      sync.Mutex
	content string
}

func (m *speechMemory) track(ctx context.Context, sub *bus.Subscription) {
	for {
		select {
		case <-ctx.Done():
			return
		case msg, ok := <-sub.C:
			if !ok {
				return
			}
			if msg.Topic != bus.TopicSense || msg.Type != bus.MsgSpeech {
				continue
			}
			content, ok := msg.Map()["content"].(string)
			if !ok {
				var bare *string
				if msg.Decode(&bare) != nil || bare == nil {
					continue
				}
				content = *bare
			}
			m.mu.Lock()
			m.content = content
			m.mu.Unlock()
		}
	}
}

func (m *speechMemory) latest() string {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.content
}

func runCycle(ctx context.Context, client *llm.Client, model string, cfg config.VisionConfig, prompt string, mc module.Ctx, memory *speechMemory) {
	slog.Debug("vision: starting capture cycle")
	// Native capture can block in the window manager. Allow shutdown while a
	// capture is in progress and buffer the result so the worker can exit.
	urls := make(chan string, 1)
	go func() {
		defer close(urls)
		img, err := capture(ctx, cfg, mc.DataDir)
		if err != nil {
			if ctx.Err() == nil {
				slog.Warn("vision: capture failed", "error", err)
			}
			return
		}
		dataURL, err := encodeDataURL(img)
		if err != nil {
			slog.Warn("vision: failed to encode captured image", "error", err)
			return
		}
		urls <- dataURL
	}()

	var dataURL string
	select {
	case <-ctx.Done():
		return
	case dataURL = <-urls:
		if dataURL == "" || ctx.Err() != nil {
			return
		}
	}
	text, err := analyzeImage(ctx, client, model, prompt, dataURL, memory.latest(), mc.Bus, cfg.PersonDetection)
	if ctx.Err() != nil {
		return
	}
	if err != nil {
		slog.Warn("vision: VLM analysis failed", "error", err)
		return
	}
	slog.Info("vision: cycle complete", "text", text)
	mc.Bus.Publish(bus.TopicSense, bus.MsgVision, map[string]string{"content": text})
}
