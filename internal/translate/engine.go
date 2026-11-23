package translate

import (
	"context"
	"log/slog"
	"slices"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
	"github.com/tik-choco/tc-npc/internal/osc"
)

const msgTranslation = "translation"

type source string

const (
	sourceUser  source = "user"
	sourceAgent source = "agent"
)

type translationEngine struct {
	bus *bus.Bus
	llm *llm.Client

	// Freeze the connection and its resolution inputs at startup. A live
	// assignment can change the model on this provider while keeping its endpoint.
	startupConfig config.Config
	model         string
	providerID    string
	oscAddress    string

	settingsMu   sync.RWMutex
	settings     config.TranslationConfig
	userHistory  utteranceHistory
	agentHistory utteranceHistory
	seq          atomic.Uint64
}

func newEngine(b *bus.Bus, cfg *config.Config) (*translationEngine, error) {
	resolved, err := cfg.Resolve(config.TaskTranslation)
	if err != nil {
		return nil, err
	}
	startup := config.Config{
		API:       cfg.API,
		Providers: slices.Clone(cfg.Providers),
		Mist:      cfg.Mist,
	}
	if cfg.DefaultRef != nil {
		ref := *cfg.DefaultRef
		startup.DefaultRef = &ref
	}
	return &translationEngine{
		bus:           b,
		llm:           llm.New(resolved.BaseURL, resolved.APIKey).WithReasoningEffort(resolved.ReasoningEffort),
		startupConfig: startup,
		model:         resolved.Model,
		providerID:    resolved.ProviderID,
		oscAddress:    cfg.VRC.OSCAddress,
		settings:      cfg.Translation,
	}, nil
}

func (e *translationEngine) resolveModel(settings config.TranslationConfig) (string, error) {
	cfg := e.startupConfig
	cfg.Translation = settings
	resolved, err := cfg.Resolve(config.TaskTranslation)
	if err != nil {
		return "", err
	}
	if resolved.ProviderID != e.providerID {
		return e.model, nil
	}
	return resolved.Model, nil
}

func (e *translationEngine) settingsSnapshot() config.TranslationConfig {
	e.settingsMu.RLock()
	defer e.settingsMu.RUnlock()
	return e.settings
}

func (e *translationEngine) updateSettings(settings config.TranslationConfig) {
	e.settingsMu.Lock()
	defer e.settingsMu.Unlock()
	e.settings = settings
}

func (e *translationEngine) translate(ctx context.Context, source source, text string) {
	if ctx.Err() != nil {
		return
	}
	settings := e.settingsSnapshot()
	history := &e.userHistory
	if source == sourceAgent {
		history = &e.agentHistory
	}
	previous := history.record(text, settings.ContextSize)

	langs, reversed := resolveTargets(settings, source, text)
	if len(langs) == 0 {
		slog.Debug("npc-translate: no target languages configured, skipping")
		return
	}

	// Both the original and the translations carry this id so the UI can show
	// the original immediately and fill in the translations as they arrive.
	entry := translationUpdate{
		ID:       "t" + strconv.FormatUint(e.seq.Add(1), 10),
		Source:   source,
		Original: text,
		Reversed: reversed,
	}
	e.publish(entry)

	model, err := e.resolveModel(settings)
	if err != nil {
		slog.Error("npc-translate: model is unavailable", "error", err)
		return
	}

	type result struct {
		lang string
		text string
		err  error
	}
	// Buffer one result per request so workers can exit even after cancellation.
	completed := make(chan result, len(langs))
	for _, lang := range langs {
		go func() {
			translated, err := e.request(ctx, model, buildPrompt(lang, previous, text))
			completed <- result{lang: lang, text: translated, err: err}
		}()
	}

	// Each chatbox update fills in the results received so far, in target order.
	results := make(map[string]string, len(langs))
	for range langs {
		select {
		case <-ctx.Done():
			return
		case result := <-completed:
			if ctx.Err() != nil {
				return
			}
			if result.err != nil {
				slog.Error("npc-translate: translation failed", "error", result.err, "lang", result.lang)
				continue
			}
			if result.text == "" {
				continue
			}
			entry.Lang, entry.Text = result.lang, result.text
			e.publish(entry)

			if settings.Chatbox {
				results[result.lang] = result.text
				chatbox := composeChatbox(text, langs, results)
				go func() {
					if ctx.Err() == nil {
						osc.SendChatbox(e.oscAddress, chatbox)
					}
				}()
			}
		}
	}
}

func (e *translationEngine) request(ctx context.Context, model, prompt string) (string, error) {
	req := llm.NewChatRequest(model, []llm.ChatMessage{llm.UserMessage(prompt)})
	response, err := e.llm.Chat(ctx, req)
	if err != nil {
		return "", err
	}
	if content := response.Content(); content != nil {
		return strings.TrimSpace(*content), nil
	}
	return "", nil
}

type translationUpdate struct {
	ID       string `json:"id"`
	Source   source `json:"source"`
	Original string `json:"original"`
	Lang     string `json:"lang"`
	Text     string `json:"text"`
	Reversed bool   `json:"reversed"`
	TS       int64  `json:"ts"`
}

func (e *translationEngine) publish(update translationUpdate) {
	update.TS = time.Now().UnixMilli()
	e.bus.Publish(bus.TopicUI, msgTranslation, update)
}

type utteranceHistory struct {
	mu    sync.Mutex
	lines []string
}

func (h *utteranceHistory) record(text string, contextSize uint32) string {
	h.mu.Lock()
	defer h.mu.Unlock()
	h.lines = pushHistory(h.lines, text, contextSize)
	return buildContext(h.lines)
}

// A zero context size still keeps the current utterance.
func pushHistory(history []string, text string, contextSize uint32) []string {
	history = append(history, text)
	keep := max(contextSize, 1)
	if uint64(len(history)) > uint64(keep) {
		history = slices.Delete(history, 0, len(history)-int(keep))
	}
	return history
}

func buildContext(history []string) string {
	if len(history) < 2 {
		return ""
	}
	var context strings.Builder
	context.WriteString("Context for translation:\n")
	for _, text := range history[:len(history)-1] {
		context.WriteString("Previous message: ")
		context.WriteString(text)
		context.WriteByte('\n')
	}
	context.WriteByte('\n')
	return context.String()
}

func buildPrompt(lang, previous, text string) string {
	return "Translate the following text to " + lang + " accurately and concisely, using the provided " +
		"context for reference but translating ONLY the final text. Return ONLY the translated " +
		"text.\n\n" + previous + "Text to translate: " + text
}

// Reverse heard speech only when it is in a configured target language.
// An unrelated language still goes to the normal targets.
func resolveTargets(settings config.TranslationConfig, source source, text string) ([]string, bool) {
	langs := settings.TargetLanguages()
	if source == sourceAgent || !settings.AutoReverse || len(langs) == 0 {
		return langs, false
	}
	back := strings.TrimSpace(settings.SourceLanguage)
	detected := detectLanguage(text)
	if detected == "" || detected == back || back == "" || !slices.Contains(langs, detected) {
		return langs, false
	}
	return []string{back}, true
}

func composeChatbox(original string, langs []string, results map[string]string) string {
	lines := make([]string, 0, len(langs)+1)
	if original = strings.TrimSpace(original); original != "" {
		lines = append(lines, original)
	}
	for _, lang := range langs {
		if translated, ok := results[lang]; ok {
			lines = append(lines, translated)
		}
	}
	return strings.Join(lines, "\n")
}
