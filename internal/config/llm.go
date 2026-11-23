package config

import (
	"fmt"
	"strings"
)

const roomScheme = "mist-network://"

// Room returns the raw room ID if this provider has a nonblank Mist room URL.
func (p *ProviderConfig) Room() (string, bool) {
	room, ok := strings.CutPrefix(p.BaseURL, roomScheme)
	if !ok || strings.TrimSpace(room) == "" {
		return "", false
	}
	return room, true
}

// LlmTask identifies the use for which a model is resolved.
type LlmTask string

const (
	TaskTalk        LlmTask = "Talk"
	TaskMemory      LlmTask = "Memory"
	TaskEmbedding   LlmTask = "Embedding"
	TaskVision      LlmTask = "Vision"
	TaskAction      LlmTask = "Action"
	TaskTranslation LlmTask = "Translation"
	TaskTTS         LlmTask = "Tts"
	TaskSTT         LlmTask = "Stt"
)

var llmTasks = [...]LlmTask{
	TaskTalk, TaskMemory, TaskEmbedding, TaskVision, TaskAction, TaskTranslation, TaskTTS, TaskSTT,
}

// ResolvedLlm is the connection and model to use for a task.
type ResolvedLlm struct {
	ProviderID      string
	BaseURL         string
	APIKey          string
	Model           string
	ReasoningEffort string
}

// Provider returns the first stored provider with the given ID, or nil.
func (c *Config) Provider(id string) *ProviderConfig {
	for i := range c.Providers {
		if c.Providers[i].ID == id {
			return &c.Providers[i]
		}
	}
	return nil
}

// TaskRef returns the stored assignment for task, or nil when it has none.
func (c *Config) TaskRef(task LlmTask) *ModelRef {
	if assignment := c.taskAssignment(task); assignment != nil {
		return *assignment
	}
	return nil
}

func (c *Config) taskAssignment(task LlmTask) **ModelRef {
	switch task {
	case TaskTalk:
		return &c.Talk.ModelRef
	case TaskMemory:
		return &c.Memory.ModelRef
	case TaskEmbedding:
		return &c.Memory.EmbeddingRef
	case TaskVision:
		return &c.Vision.ModelRef
	case TaskAction:
		return &c.Action.ModelRef
	case TaskTranslation:
		return &c.Translation.ModelRef
	case TaskTTS:
		return &c.TTS.ModelRef
	case TaskSTT:
		return &c.STT.ModelRef
	default:
		return nil
	}
}

func (c *Config) taskEffort(task LlmTask) string {
	var effort string
	switch task {
	case TaskTalk:
		effort = c.Talk.ReasoningEffort
	case TaskMemory:
		effort = c.Memory.ReasoningEffort
	case TaskVision:
		effort = c.Vision.ReasoningEffort
	case TaskAction:
		effort = c.Action.ReasoningEffort
	case TaskTranslation:
		effort = c.Translation.ReasoningEffort
	}
	return firstNonempty(effort, c.API.ReasoningEffort)
}

// Resolve selects the exact assignment or the default when the task has no
// assignment or its provider is disabled. It leaves stored assignments intact.
func (c *Config) Resolve(task LlmTask) (ResolvedLlm, error) {
	assignment := c.taskAssignment(task)
	if assignment == nil {
		return ResolvedLlm{}, fmt.Errorf("unknown LLM task %q", task)
	}
	reference := *assignment
	if reference == nil {
		reference = c.DefaultRef
	}
	if reference == nil {
		return ResolvedLlm{}, fmt.Errorf("%s: no model reference configured", task)
	}
	provider := c.Provider(reference.ProviderID)
	if provider == nil {
		return ResolvedLlm{}, fmt.Errorf("%s: provider %q is missing", task, reference.ProviderID)
	}
	if !provider.Enabled {
		reference = c.DefaultRef
		if reference == nil {
			return ResolvedLlm{}, fmt.Errorf("%s: provider is disabled and no default is configured", task)
		}
		provider = c.Provider(reference.ProviderID)
		if provider == nil {
			return ResolvedLlm{}, fmt.Errorf("%s: default provider is missing", task)
		}
	}
	if !provider.Enabled {
		return ResolvedLlm{}, fmt.Errorf("%s: default provider is disabled", task)
	}
	if strings.TrimSpace(reference.Model) == "" {
		return ResolvedLlm{}, fmt.Errorf("%s: model is empty", task)
	}
	if strings.TrimSpace(provider.BaseURL) == "" {
		return ResolvedLlm{}, fmt.Errorf("%s: provider URL is empty", task)
	}
	resolved := ResolvedLlm{
		ProviderID:      provider.ID,
		BaseURL:         provider.BaseURL,
		APIKey:          provider.APIKey,
		Model:           reference.Model,
		ReasoningEffort: c.taskEffort(task),
	}
	if strings.HasPrefix(provider.BaseURL, roomScheme) {
		room, ok := provider.Room()
		if !ok {
			return ResolvedLlm{}, fmt.Errorf("%s: room ID is empty", task)
		}
		if strings.TrimSpace(c.Mist.AIBaseURL) == "" {
			return ResolvedLlm{}, fmt.Errorf("%s: mistl AI serve URL is empty", task)
		}
		resolved.BaseURL = roomBaseURL(c.Mist.AIBaseURL, room)
		resolved.APIKey = ""
	}
	return resolved, nil
}

func roomBaseURL(base, room string) string {
	// The bridge expects one path segment, escaping every non-unreserved byte.
	const hex = "0123456789ABCDEF"
	var url strings.Builder
	url.WriteString(strings.TrimRight(base, "/"))
	url.WriteString("/rooms/")
	for i := 0; i < len(room); i++ {
		b := room[i]
		switch {
		case b >= 'a' && b <= 'z', b >= 'A' && b <= 'Z', b >= '0' && b <= '9',
			b == '-', b == '.', b == '_', b == '~':
			url.WriteByte(b)
		default:
			url.WriteByte('%')
			url.WriteByte(hex[b>>4])
			url.WriteByte(hex[b&0xf])
		}
	}
	return url.String()
}

func firstNonempty(value, fallback string) string {
	if value != "" {
		return value
	}
	return fallback
}
