// Package talk handles conversation turns on the NPC bus.
package talk

import (
	"bytes"
	"context"
	"log/slog"
	"strings"
	"sync"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/module"
)

const (
	suspendTimeout       = 600 * time.Second
	absenceCheckInterval = 15 * time.Second
)

type talkModule struct{}

// New returns the NPC talk module. Its LLM connection is resolved at startup.
func New(_ module.Ctx) (module.Module, error) { return talkModule{}, nil }

func (talkModule) Name() string { return "npc-talk" }

func (talkModule) Run(ctx context.Context, mc module.Ctx) error {
	cfg := mc.Config
	resolved, err := cfg.Resolve(config.TaskTalk)
	if err != nil {
		return err
	}
	client := llm.New(resolved.BaseURL, resolved.APIKey).WithReasoningEffort(resolved.ReasoningEffort)
	active, _ := character.ActiveCharacter(mc.DataDir, cfg.Character.ActiveID)
	engine := NewChatEngine(client, resolved.Model, cfg.Talk, ToolRegistry{}, characterPersona(active), cfg.Language, mc.Bus)

	sub := mc.Bus.Subscribe()
	defer sub.Close()
	state := talkState{
		interpretMode:     cfg.Translation.SuppressesChat(),
		activeCharacterID: cfg.Character.ActiveID,
	}

	runCtx, cancel := context.WithCancel(ctx)
	var workers sync.WaitGroup
	defer func() {
		cancel()
		workers.Wait()
	}()

	// Keep bus dispatch responsive during LLM calls. Apply queued state
	// updates in bus order, so a switch takes effect on the next turn.
	work := make(chan func())
	workers.Go(func() {
		for {
			select {
			case <-runCtx.Done():
				return
			case run := <-work:
				if runCtx.Err() != nil {
					return
				}
				run()
			}
		}
	})
	var pending []func()

	// Stream-event name preparation runs before acquiring the turn lock,
	// and other turns may proceed while the pronunciation is being fetched.
	prepared := make(chan func())
	absenceCheck := time.NewTicker(absenceCheckInterval)
	defer absenceCheck.Stop()
	resume := time.NewTimer(suspendTimeout)
	resume.Stop()
	defer resume.Stop()
	var resumeC <-chan time.Time
	var dropped uint64

	for {
		var ready chan<- func()
		var job func()
		if len(pending) > 0 {
			ready, job = work, pending[0]
		}
		select {
		case <-runCtx.Done():
			slog.Info("npc-talk: shutdown requested, stopping")
			return nil
		case ready <- job:
			pending[0] = nil
			pending = pending[1:]
		case job := <-prepared:
			pending = append(pending, job)
		case <-absenceCheck.C:
			engine.CheckPartnerAbsence()
		case <-resumeC:
			slog.Debug("npc-talk: auto-resuming after suspend timeout")
			state.suspended = false
			state.resumeDeadline = time.Time{}
			resumeC = nil
		case msg, ok := <-sub.C:
			if !ok {
				slog.Warn("npc-talk: bus channel closed, stopping")
				return nil
			}
			if count := sub.Dropped(); count != dropped {
				slog.Warn("npc-talk: bus receiver lagged, messages were dropped", "skipped", count-dropped)
				dropped = count
			}
			previousDeadline := state.resumeDeadline
			action := handleBusMessage(engine, &state, msg, mc.DataDir)
			if state.resumeDeadline != previousDeadline {
				resume.Stop()
				resumeC = nil
				if !state.resumeDeadline.IsZero() {
					resume.Reset(time.Until(state.resumeDeadline))
					resumeC = resume.C
				}
			}
			if action.update != nil {
				pending = append(pending, action.update)
			}
			if input := action.speech; input != nil {
				run := func() { input.run(runCtx, engine) }
				if input.transliterate && hasLatinLetters(stringOrEmpty(input.speaker)) {
					workers.Go(func() {
						input.content = maybeTransliterateEventName(runCtx, engine, input.content, input.speaker)
						select {
						case prepared <- run:
						case <-runCtx.Done():
						}
					})
				} else {
					pending = append(pending, run)
				}
			}
		}
	}
}

type talkState struct {
	suspended         bool
	resumeDeadline    time.Time
	interpretMode     bool
	activeCharacterID string
}

type busAction struct {
	update func()
	speech *speechInput
}

type speechInput struct {
	content       string
	speaker       *string
	requestID     *string
	transliterate bool
}

func (input speechInput) run(ctx context.Context, engine *ChatEngine) {
	if _, err := engine.ChatWithContext(ctx, input.content, input.speaker, input.requestID); err != nil {
		slog.Error("npc-talk: chat turn failed", "error", err)
	}
}

func handleBusMessage(engine *ChatEngine, state *talkState, msg bus.Message, dataDir string) busAction {
	switch msg.Topic {
	case bus.TopicMem:
		var update func(string)
		var kind string
		switch msg.Type {
		case bus.MsgShortTermMemory:
			update, kind = engine.SetShortTermMemory, "short term memory"
		case bus.MsgLongTermMemory:
			update, kind = engine.SetLongTermMemory, "long term memory"
		case bus.MsgPersonMemory:
			update, kind = engine.SetPersonMemory, "person memory"
		}
		if update != nil {
			if content := extractContent(msg); content != nil {
				slog.Info("npc-talk: updated "+kind, "content", *content)
				return busAction{update: func() { update(*content) }}
			}
		}
	case bus.TopicSense:
		if msg.Type != bus.MsgSpeech {
			break
		}
		content := extractContent(msg)
		if content == nil {
			break
		}
		if state.interpretMode {
			slog.Debug("npc-talk: interpretation mode, leaving speech to npc-translate", "content", *content)
		} else if state.suspended {
			slog.Debug("npc-talk: suspended, dropping speech input", "content", *content)
		} else {
			payload := msg.Map()
			input := &speechInput{
				content:       *content,
				speaker:       payloadString(payload, "speaker"),
				requestID:     payloadString(payload, "request_id"),
				transliterate: extractTransliterateName(payload),
			}
			slog.Info("npc-talk: received speech input", "content", *content,
				"speaker", stringOrEmpty(input.speaker), "request_id", stringOrEmpty(input.requestID))
			return busAction{speech: input}
		}
	case bus.TopicInterrupt:
		switch msg.Type {
		case bus.MsgSuspend:
			state.suspended = true
			state.resumeDeadline = time.Now().Add(suspendTimeout)
			slog.Info("npc-talk: suspended (auto-resume in 600s)")
		case bus.MsgResume:
			state.suspended = false
			state.resumeDeadline = time.Time{}
			slog.Info("npc-talk: resumed")
		}
	case bus.TopicConfig:
		if msg.Type != bus.MsgConfigUpdated {
			break
		}
		var cfg config.Config
		if err := msg.Decode(&cfg); err != nil {
			slog.Warn("npc-talk: failed to deserialize updated config, keeping current mode", "error", err)
			break
		}
		if interpret := cfg.Translation.SuppressesChat(); interpret != state.interpretMode {
			slog.Info("npc-talk: interpretation mode changed", "interpret_mode", interpret)
			state.interpretMode = interpret
		}
		// Config updates also carry unrelated saves. Only reload a changed ID.
		if id := cfg.Character.ActiveID; id != state.activeCharacterID {
			oldID := state.activeCharacterID
			state.activeCharacterID = id
			return busAction{update: func() { applyCharacterSwitch(engine, dataDir, id, oldID) }}
		}
	}
	return busAction{}
}

func characterPersona(c *character.Character) *string {
	if c == nil {
		return nil
	}
	persona := character.PersonaPrompt(&c.Sheet)
	return &persona
}

func applyCharacterSwitch(engine *ChatEngine, dataDir, activeID, oldID string) {
	active, _ := character.ActiveCharacter(dataDir, activeID)
	from := oldID
	if from == "" {
		from = "(none)"
	}
	switch {
	case active != nil:
		slog.Info("npc-talk: active character switched, persona reloaded", "from", from, "to", active.Sheet.Name)
	case activeID == "":
		slog.Info("npc-talk: active character cleared, persona reset to none", "from", from)
	default:
		slog.Warn("npc-talk: active character could not be loaded, clearing persona", "from", from, "active_id", activeID)
	}
	engine.SetPersona(characterPersona(active))
}

// Memory and speech payloads may be an object or a bare JSON string.
func extractContent(msg bus.Message) *string {
	if bytes.HasPrefix(bytes.TrimSpace(msg.Payload), []byte{'"'}) {
		var content string
		if msg.Decode(&content) == nil {
			return &content
		}
	}
	return payloadString(msg.Map(), "content")
}

// An explicit empty string is valid, including for a requester ID.
func payloadString(payload map[string]any, key string) *string {
	value, ok := payload[key].(string)
	if !ok {
		return nil
	}
	return &value
}

func extractTransliterateName(payload map[string]any) bool {
	value, _ := payload["transliterate_name"].(bool)
	return value
}

func maybeTransliterateEventName(ctx context.Context, engine *ChatEngine, content string, speaker *string) string {
	if speaker == nil || !hasLatinLetters(*speaker) {
		return content
	}
	// Put the katakana in the event instruction so the model can say it.
	return strings.ReplaceAll(content, *speaker, engine.TransliterateName(ctx, *speaker))
}
