// Package action dispatches movement commands and natural-language plans.
package action

import (
	"context"
	"fmt"
	"log/slog"
	"sync"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/module"
)

const msgCommand = "command"

type actionModule struct{}

// New constructs the npc-action module. Run connects the actuator and LLM
// using the startup config.
func New(_ module.Ctx) (module.Module, error) {
	return actionModule{}, nil
}

func (actionModule) Name() string { return "npc-action" }

func (actionModule) Run(ctx context.Context, mc module.Ctx) error {
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()

	cfg := mc.Config
	// Only map data is reloaded. Actuator and LLM settings come from startup.
	body, err := connectVRC(ctx, cfg.Action.OSCAddress)
	if err != nil {
		if ctx.Err() != nil {
			return nil
		}
		return err
	}
	defer body.socket.Close()

	resolved, err := cfg.Resolve(config.TaskAction)
	if err != nil {
		return fmt.Errorf("resolve action LLM: %w", err)
	}
	prompt := buildStaticSystemPrompt(cfg.Language)
	nav := &navigator{body: body, bus: mc.Bus}
	state := &actionState{
		bus:          mc.Bus,
		shutdown:     ctx,
		body:         body,
		controller:   controller{body: body},
		navigator:    nav,
		autopilot:    autopilot{nav: nav},
		data:         mapDataFromConfig(cfg),
		llm:          llm.New(resolved.BaseURL, resolved.APIKey).WithReasoningEffort(resolved.ReasoningEffort),
		model:        resolved.Model,
		staticPrompt: prompt,
		historySize:  cfg.Talk.HistorySize,
		history:      []llm.ChatMessage{llm.SystemMessage(prompt)},
	}

	var chats sync.WaitGroup
	startChat := func(text string) {
		chats.Go(func() { state.chatWithAction(text) })
	}
	defer func() {
		cancel()
		state.stopActiveTask()
		chats.Wait()
	}()

	sub := mc.Bus.Subscribe()
	defer sub.Close()
	slog.Info("npc-action: OSC client ready", "addr", cfg.Action.OSCAddress)
	var dropped uint64
	for ctx.Err() == nil {
		select {
		case <-ctx.Done():
			return nil
		case msg, ok := <-sub.C:
			if !ok {
				return nil
			}
			if total := sub.Dropped(); total != dropped {
				slog.Warn("npc-action: bus receiver lagged, some messages dropped", "skipped", total-dropped)
				dropped = total
			}
			switch msg.Topic {
			case bus.TopicConfig:
				if msg.Type == bus.MsgConfigUpdated {
					state.handleConfigUpdated(msg)
				}
			case bus.TopicAction:
				handleActionMessage(state, msg, startChat)
			}
		}
	}
	return nil
}

func (s *actionState) handleConfigUpdated(msg bus.Message) {
	var cfg config.Config
	if err := msg.Decode(&cfg); err != nil {
		slog.Warn("npc-action: failed to deserialize updated config, keeping current locations/routes", "error", err)
		return
	}
	data := mapDataFromConfig(&cfg)
	s.replaceMap(data)
	slog.Info("npc-action: config updated, locations/routes reloaded", "locations", len(data.locations), "routes", len(data.routes))
}

func handleActionMessage(state *actionState, msg bus.Message, startChat func(string)) {
	switch msg.Type {
	case bus.MsgAction:
		var text *string
		if msg.Decode(&text) == nil && text != nil {
			startChat(*text)
		} else if content, ok := msg.Map()["content"].(string); ok {
			startChat(content)
		} else {
			slog.Warn("npc-action: 'action' message missing text/content payload")
		}
	case msgCommand:
		text, ok := msg.Map()["text"].(string)
		if !ok {
			slog.Warn("npc-action: 'command' message missing text payload")
			return
		}
		if !state.dispatchCommand(text) {
			startChat(text)
		}
	default:
		slog.Debug("npc-action: ignoring unrecognized bus message type", "msg_type", msg.Type)
	}
}
