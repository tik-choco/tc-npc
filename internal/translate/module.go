// Package translate subtitles heard speech and NPC replies as configured.
package translate

import (
	"context"
	"log/slog"
	"strings"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/module"
)

type translateModule struct{}

// New constructs npc-translate even when translation is disabled, so it can
// pick up live config changes.
func New(_ module.Ctx) (module.Module, error) {
	return translateModule{}, nil
}

func (translateModule) Name() string { return "npc-translate" }

func (translateModule) Run(ctx context.Context, mc module.Ctx) error {
	engine, err := newEngine(mc.Bus, mc.Config)
	if err != nil {
		return err
	}
	sub := mc.Bus.Subscribe()
	defer sub.Close()
	logSettings(mc.Config.Translation)

	var dropped uint64
	for {
		select {
		case <-ctx.Done():
			slog.Info("npc-translate: shutting down")
			return nil
		case msg, ok := <-sub.C:
			if !ok {
				slog.Info("npc-translate: bus closed, shutting down")
				return nil
			}
			if total := sub.Dropped(); total != dropped {
				slog.Warn("npc-translate: bus receiver lagged, some messages dropped", "skipped", total-dropped)
				dropped = total
			}
			handleBusMessage(ctx, engine, msg)
		}
	}
}

func logSettings(settings config.TranslationConfig) {
	mode, scope := settings.Mode(), settings.Scope()
	switch {
	case mode.IsOff():
		slog.Info("npc-translate: mode=off, idling until config update")
	case mode == config.TranslationModeInterpret && scope == config.TranslationScopeOutput:
		// This valid pairing translates nothing because interpret mode suppresses replies.
		slog.Warn("npc-translate: scope=output with mode=interpret — the NPC does not reply in this mode, so nothing will be translated")
	default:
		slog.Info("npc-translate: interpretation active", "mode", mode, "scope", scope,
			"source", settings.SourceLanguage, "targets", settings.TargetLanguages())
	}
}

func handleBusMessage(ctx context.Context, engine *translationEngine, msg bus.Message) {
	switch {
	case msg.Topic == bus.TopicConfig && msg.Type == bus.MsgConfigUpdated:
		var updated config.Config
		if err := msg.Decode(&updated); err != nil {
			slog.Warn("npc-translate: failed to deserialize updated config, keeping current settings", "error", err)
			return
		}
		engine.updateSettings(updated.Translation)
		slog.Info("npc-translate: config updated")
		logSettings(updated.Translation)
	case msg.Topic == bus.TopicSense && msg.Type == bus.MsgSpeech:
		settings := engine.settingsSnapshot()
		if !settings.Mode().IsOff() && settings.Scope().TranslatesInput() {
			spawnTranslation(ctx, engine, sourceUser, msg)
		}
	case msg.Topic == bus.TopicChat && msg.Type == bus.MsgChatResponse:
		// npc-talk suppresses replies in interpret mode. Only assist subtitles them.
		settings := engine.settingsSnapshot()
		if settings.Mode() == config.TranslationModeAssist && settings.Scope().TranslatesOutput() {
			spawnTranslation(ctx, engine, sourceAgent, msg)
		}
	}
}

func spawnTranslation(ctx context.Context, engine *translationEngine, source source, msg bus.Message) {
	text, ok := extractContent(msg)
	if ok && strings.TrimSpace(text) != "" {
		go engine.translate(ctx, source, text)
	}
}

// Speech and chat responses may be a bare JSON string or {"content": "..."}.
func extractContent(msg bus.Message) (string, bool) {
	var bare *string
	if err := msg.Decode(&bare); err == nil && bare != nil {
		return *bare, true
	}
	content, ok := msg.Map()["content"].(string)
	return content, ok
}
