package talk

import (
	"context"
	"errors"
	"log/slog"
	"maps"
	"math"
	"regexp"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/chatlog"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
)

type chatState struct {
	// History is shared by everyone in the conversation. Drives follow the speaker.
	history         []llm.ChatMessage
	shortTermMemory string
	longTermMemory  string
	personMemory    string
	persona         *string
	affect          PartnerAffect
}

type chatClient interface {
	Chat(context.Context, llm.ChatRequest) (llm.ChatResponse, error)
}

// ChatEngine composes LLM turns and publishes their outcomes.
type ChatEngine struct {
	llm                 chatClient
	model               string
	prompts             []config.PromptEntry
	personaReferenced   bool
	filterPrompt        *string
	historySize         int
	tools               ToolRegistry
	bus                 *bus.Bus
	languageInstruction string
	styleRules          bool
	allowSilence        bool
	affectEnabled       bool
	affectForcedClosure bool
	absenceTimeout      time.Duration

	// A turn holds this lock across prompt assembly, LLM calls, and history.
	mu    sync.Mutex
	state chatState
}

// NewChatEngine takes a startup snapshot of the talk settings and tool registry.
func NewChatEngine(client *llm.Client, model string, cfg config.TalkConfig, tools ToolRegistry, persona *string, language string, b *bus.Bus) *ChatEngine {
	instruction, _ := config.LanguageInstruction(language)
	e := &ChatEngine{
		llm:                 client,
		model:               model,
		prompts:             slices.Clone(cfg.Prompts),
		historySize:         int(min(uint64(cfg.HistorySize), uint64(math.MaxInt))),
		tools:               ToolRegistry{tools: maps.Clone(tools.tools)},
		bus:                 b,
		languageInstruction: instruction,
		styleRules:          cfg.StyleRules,
		allowSilence:        cfg.AllowSilence,
		affectEnabled:       cfg.Affect.Enabled,
		affectForcedClosure: cfg.Affect.ForcedClosure,
		absenceTimeout:      time.Duration(min(cfg.Affect.AbsenceTimeoutSecs, math.MaxInt64/uint64(time.Second))) * time.Second,
		state:               chatState{persona: copyString(persona), affect: NewPartnerAffect()},
	}
	for _, prompt := range e.prompts {
		if cfg.FilterPromptName != "" && prompt.Name == cfg.FilterPromptName && e.filterPrompt == nil {
			e.filterPrompt = &prompt.Content
		}
		if strings.Contains(prompt.Content, "{{persona}}") {
			e.personaReferenced = true
		}
	}
	e.publishAffectSnapshot()
	return e
}

func (e *ChatEngine) publishAffectSnapshot() {
	if e.affectEnabled {
		e.bus.Publish(bus.TopicUI, bus.MsgAffectState, e.state.affect.Snapshot())
	}
}

// CheckPartnerAbsence applies the idle reset. The next poll or turn takes care
// of it if a turn is already in progress.
func (e *ChatEngine) CheckPartnerAbsence() {
	if !e.affectEnabled || e.absenceTimeout == 0 || !e.mu.TryLock() {
		return
	}
	defer e.mu.Unlock()
	if absence := e.state.affect.CheckAbsence(time.Now(), e.absenceTimeout); absence != nil {
		slog.Info("npc-talk: conversation partner went quiet, treating them as away",
			"who", stringOrEmpty(absence.Who), "idle_secs", absence.Idle/time.Second)
		e.publishAffectSnapshot()
	}
}

func (e *ChatEngine) SetShortTermMemory(content string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.state.shortTermMemory = content
}

func (e *ChatEngine) SetLongTermMemory(content string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.state.longTermMemory = content
}

func (e *ChatEngine) SetPersonMemory(content string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.state.personMemory = content
}

// SetPersona applies a live character switch once the current turn finishes.
func (e *ChatEngine) SetPersona(persona *string) {
	e.mu.Lock()
	defer e.mu.Unlock()
	e.state.persona = copyString(persona)
}

func (e *ChatEngine) Chat(ctx context.Context, input string) (string, error) {
	return e.ChatWithSpeaker(ctx, input, nil)
}

func (e *ChatEngine) ChatWithSpeaker(ctx context.Context, input string, speaker *string) (string, error) {
	return e.ChatWithContext(ctx, input, speaker, nil)
}

// ChatWithContext carries the requester ID with this turn through every outcome.
// A nil ID represents a turn with no requester.
func (e *ChatEngine) ChatWithContext(ctx context.Context, input string, speaker, requestID *string) (string, error) {
	result, err := e.runTurn(ctx, input, speaker, requestID)
	if err != nil && requestID != nil {
		e.bus.Publish(bus.TopicChat, bus.MsgChatError, map[string]string{
			"request_id": *requestID,
			"message":    err.Error(),
		})
	}
	return result, err
}

func (e *ChatEngine) runTurn(ctx context.Context, input string, speaker, requestID *string) (string, error) {
	if !e.mu.TryLock() {
		slog.Debug("npc-talk: chat turn queued behind an in-progress turn")
		e.mu.Lock()
	}
	defer e.mu.Unlock()
	if err := ctx.Err(); err != nil {
		return "", err
	}
	state := &e.state
	user := llm.UserMessage(input)
	if e.affectEnabled {
		// Catch up on absence before scoring the utterance against its speaker.
		now := time.Now()
		state.affect.CheckAbsence(now, e.absenceTimeout)
		if change := state.affect.Observe(speaker); change != nil {
			slog.Info("npc-talk: conversation partner changed", "from", stringOrEmpty(change.From), "to", change.To, "returning", change.Returning)
		}
		state.affect.Update(input, now)
		e.publishAffectSnapshot()
		if e.affectForcedClosure {
			if reply, ok := state.affect.state().ForcedClosureReply(input); ok {
				state.history = append(state.history, user)
				return e.finishTurn(input, reply, "closing", speaker, requestID), nil
			}
		}
	}

	vars := buildVars(state.shortTermMemory, state.longTermMemory, state.personMemory)
	persona := stringOrEmpty(state.persona)
	vars["persona"] = persona
	var messages []llm.ChatMessage
	if !e.personaReferenced && persona != "" {
		messages = append(messages, llm.SystemMessage(persona))
	}
	for _, prompt := range e.prompts {
		if content := fillTemplate(prompt.Content, vars); content != "" {
			messages = append(messages, llm.SystemMessage(content))
		}
	}
	if rules := e.styleMessage(); rules != nil {
		messages = append(messages, *rules)
	}
	if e.affectEnabled {
		messages = append(messages, llm.SystemMessage(state.affect.state().ToPrompt()))
	}
	if e.languageInstruction != "" {
		messages = append(messages, llm.SystemMessage(e.languageInstruction))
	}
	messages = append(messages, state.history...)
	messages = append(messages, user)

	toolDefs := e.tools.Definitions()
	response, err := e.chatWithTools(ctx, messages, toolDefs)
	if err != nil {
		return "", err
	}
	state.history = append(state.history, user)

	for len(response.ToolCalls) != 0 {
		assistant := llm.AssistantMessage(stringOrEmpty(response.Content))
		assistant.ToolCalls = response.ToolCalls
		messages = append(messages, assistant)
		state.history = append(state.history, assistant)
		for _, call := range response.ToolCalls {
			slog.Info("npc-talk: tool call", "name", call.Function.Name, "args", call.Function.Arguments)
			result := e.tools.Execute(ctx, call.Function.Name, call.Function.Arguments)
			toolMessage := llm.ToolResultMessage(call.ID, result)
			messages = append(messages, toolMessage)
			state.history = append(state.history, toolMessage)
		}
		response, err = e.chatWithTools(ctx, messages, toolDefs)
		if err != nil {
			return "", err
		}
	}

	reply := stringOrEmpty(response.Content)
	if e.filterPrompt != nil && reply != "" {
		// A rewrite needs the same output language as the original turn.
		system := *e.filterPrompt
		if e.languageInstruction != "" {
			system += "\n\n" + e.languageInstruction
		}
		filtered, err := e.chatWithTools(ctx, []llm.ChatMessage{llm.SystemMessage(system), llm.UserMessage(reply)}, nil)
		if err != nil {
			slog.Warn("npc-talk: filter pass failed, keeping unfiltered response", "error", err)
		} else {
			reply = stringOrEmpty(filtered.Content)
		}
	}
	// Sanitize after filtering so stage directions never reach TTS or history.
	return e.finishTurn(input, SanitizeReply(reply), "declined", speaker, requestID), nil
}

func (e *ChatEngine) finishTurn(input, reply, silenceReason string, speaker, requestID *string) string {
	if e.allowSilence && IsSilence(reply) {
		e.state.history = trimHistory(e.state.history, e.historySize)
		slog.Info("npc-talk: staying silent this turn", "reason", silenceReason, "input", input, "request_id", stringOrEmpty(requestID))
		e.bus.Publish(bus.TopicChat, bus.MsgChatSilent, chatSilentPayload(input, silenceReason, requestID))
		e.bus.Publish(bus.TopicChat, bus.MsgChatLog, chatLogPayload(input, "", speaker))
		return ""
	}
	if reply == "" {
		reply = " "
	}
	e.state.history = append(e.state.history, llm.AssistantMessage(reply))
	e.state.history = trimHistory(e.state.history, e.historySize)
	e.bus.Publish(bus.TopicChat, bus.MsgChatResponse, chatResponsePayload(reply, input, requestID))
	e.bus.Publish(bus.TopicChat, bus.MsgChatLog, chatLogPayload(input, reply, speaker))
	return reply
}

func (e *ChatEngine) chatWithTools(ctx context.Context, messages []llm.ChatMessage, tools []llm.Tool) (llm.ResponseMessage, error) {
	request := llm.NewChatRequest(e.model, messages)
	request.Tools = tools
	response, err := e.llm.Chat(ctx, request)
	if err != nil {
		return llm.ResponseMessage{}, err
	}
	if len(response.Choices) == 0 {
		return llm.ResponseMessage{}, errors.New("no choices returned")
	}
	return response.Choices[0].Message, nil
}

func (e *ChatEngine) styleMessage() *llm.ChatMessage {
	if !e.styleRules {
		return nil
	}
	message := llm.SystemMessage(SpokenReplyRules)
	return &message
}

func buildVars(shortTermMemory, longTermMemory, personMemory string) map[string]string {
	vars := map[string]string{"session_meta": time.Now().Format("2006-01-02 15:04:05 (Mon)")}
	if shortTermMemory != "" {
		vars["short_term_memory"] = shortTermMemory
	}
	if longTermMemory != "" {
		vars["long_term_memory"] = longTermMemory
	}
	if personMemory != "" {
		vars["person_memory"] = personMemory
	}
	return vars
}

const transliterateNamePrompt = "Transliterate the given name into Japanese katakana for text-to-speech. Respond with the katakana only, no quotes, no explanation, no extra text."

// TransliterateName reads a stream-event viewer handle in katakana for TTS.
// It uses the lowest reasoning effort and falls back to the original on failure.
func (e *ChatEngine) TransliterateName(ctx context.Context, name string) string {
	request := llm.NewChatRequest(e.model, []llm.ChatMessage{
		llm.SystemMessage(transliterateNamePrompt), llm.UserMessage(name),
	})
	effort := "none"
	request.ReasoningEffort = &effort
	response, err := e.llm.Chat(ctx, request)
	if err != nil {
		slog.Warn("npc-talk: name transliteration failed, using original name", "error", err, "name", name)
		return name
	}
	katakana := strings.TrimSpace(stringOrEmpty(response.Content()))
	if katakana == "" {
		return name
	}
	return katakana
}

func hasLatinLetters(name string) bool { return strings.ContainsFunc(name, asciiLetter) }

func stringOrEmpty(value *string) string {
	if value == nil {
		return ""
	}
	return *value
}

func chatLogPayload(input, output string, speaker *string) chatlog.ChatLogEntry {
	now := time.Now().UTC()
	// Chrono's AutoSi format uses zero, three, six or nine fractional digits.
	layout := "2006-01-02T15:04:05"
	switch ns := now.Nanosecond(); {
	case ns == 0:
	case ns%1_000_000 == 0:
		layout += ".000"
	case ns%1_000 == 0:
		layout += ".000000"
	default:
		layout += ".000000000"
	}
	return chatlog.ChatLogEntry{Time: now.Format(layout + "-07:00"), Speaker: speaker, Input: input, Output: output}
}

type chatResponse struct {
	Content   string  `json:"content"`
	Input     string  `json:"input"`
	RequestID *string `json:"request_id,omitempty"`
}

func chatResponsePayload(content, input string, requestID *string) chatResponse {
	return chatResponse{Content: content, Input: input, RequestID: requestID}
}

type chatSilence struct {
	Reason    string  `json:"reason"`
	Input     string  `json:"input"`
	RequestID *string `json:"request_id,omitempty"`
}

func chatSilentPayload(input, reason string, requestID *string) chatSilence {
	return chatSilence{Reason: reason, Input: input, RequestID: requestID}
}

var placeholder = regexp.MustCompile(`\{\{([a-zA-Z0-9_-]+)\}\}`)

func fillTemplate(template string, vars map[string]string) string {
	return strings.TrimSpace(placeholder.ReplaceAllStringFunc(template, func(match string) string {
		return vars[match[2:len(match)-2]]
	}))
}

// Prefer a user boundary within the last size messages. If there is none,
// keep the extra history up to twice the configured size.
func trimHistory(history []llm.ChatMessage, size int) []llm.ChatMessage {
	if len(history) <= size {
		return history
	}
	start := len(history) - size
	for i := start; i < len(history); i++ {
		if history[i].Role == "user" {
			return slices.Delete(history, 0, i)
		}
	}
	if start > size {
		return slices.Delete(history, 0, start)
	}
	return history
}
