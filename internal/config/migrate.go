package config

import (
	"errors"
	"fmt"
	"log/slog"
	"os"
	"path/filepath"
	"slices"
)

// MigrateLLMConfig converts legacy presets and direct connection fields to
// providers and model refs. Existing assignments and effort settings win.
// The result reports whether the canonical config or legacy fields changed.
func (c *Config) MigrateLLMConfig() bool {
	presets, defaultID := c.Presets, c.DefaultPresetID
	c.Presets, c.DefaultPresetID = []PresetConfig{}, ""
	changed := len(presets) != 0 || defaultID != ""

	direct := !changed && len(c.Providers) == 0 && c.DefaultRef == nil
	for _, task := range llmTasks {
		direct = direct && c.TaskRef(task) == nil
	}
	if direct {
		c.migrateDirectFields()
	}
	if c.DefaultRef == nil {
		c.DefaultRef = presetRef(presets, defaultID)
	}
	for _, entry := range []struct {
		task   LlmTask
		id     *string
		effort *string
	}{
		{TaskTalk, &c.Talk.PresetID, &c.Talk.ReasoningEffort},
		{TaskMemory, &c.Memory.PresetID, &c.Memory.ReasoningEffort},
		{TaskEmbedding, &c.Memory.EmbeddingPresetID, nil},
		{TaskVision, &c.Vision.PresetID, &c.Vision.ReasoningEffort},
		{TaskAction, &c.Action.PresetID, &c.Action.ReasoningEffort},
		{TaskTranslation, &c.Translation.PresetID, &c.Translation.ReasoningEffort},
		{TaskTTS, &c.TTS.PresetID, nil},
		{TaskSTT, &c.STT.PresetID, nil},
	} {
		id := *entry.id
		changed = changed || id != ""
		assignment := c.taskAssignment(entry.task)
		if *assignment == nil {
			*assignment = presetRef(presets, id)
		}
		if entry.effort != nil && *entry.effort == "" {
			if preset := findPreset(presets, firstNonempty(id, defaultID)); preset != nil {
				*entry.effort = preset.ReasoningEffort
			}
		}
		*entry.id = ""
	}
	for _, preset := range presets {
		c.cacheModel(preset.ProviderID, preset.Model)
	}
	if direct {
		if c.DefaultRef != nil {
			c.cacheModel(c.DefaultRef.ProviderID, c.DefaultRef.Model)
		}
		for _, task := range llmTasks {
			if ref := c.TaskRef(task); ref != nil {
				c.cacheModel(ref.ProviderID, ref.Model)
			}
		}
	}
	return changed || direct
}

func findPreset(presets []PresetConfig, id string) *PresetConfig {
	for i := range presets {
		if presets[i].ID == id {
			return &presets[i]
		}
	}
	return nil
}

func presetRef(presets []PresetConfig, id string) *ModelRef {
	if id == "" {
		return nil
	}
	if preset := findPreset(presets, id); preset != nil {
		return &ModelRef{ProviderID: preset.ProviderID, Model: preset.Model}
	}
	// Keep unresolved assignments visible for users to correct.
	return &ModelRef{Model: id}
}

func (c *Config) cacheModel(providerID, model string) {
	if p := c.Provider(providerID); p != nil && model != "" {
		if _, room := p.Room(); !room && !slices.Contains(p.Models, model) {
			p.Models = append(p.Models, model)
		}
	}
}

func (c *Config) migrateDirectFields() {
	c.addLegacyProvider("default", "Default", c.API.BaseURL, c.API.APIKey)
	if c.API.Model != "" {
		c.DefaultRef = &ModelRef{ProviderID: "default", Model: c.API.Model}
	}
	for _, entry := range []struct {
		task            LlmTask
		model, url, key string
		usesChatModel   bool
	}{
		{task: TaskEmbedding, model: c.API.EmbeddingModel},
		{TaskVision, c.Vision.Model, c.Vision.BaseURL, c.Vision.APIKey, true},
		{TaskTTS, c.TTS.Model, c.TTS.BaseURL, c.TTS.APIKey, true},
		// STT used its own model even when the endpoint inherited chat credentials.
		{TaskSTT, c.STT.Model, c.STT.BaseURL, c.STT.APIKey, false},
		{task: TaskTranslation, model: c.Translation.Model},
	} {
		if entry.model == "" && entry.url == "" && entry.key == "" {
			continue
		}
		url := firstNonempty(entry.url, c.API.BaseURL)
		key := firstNonempty(entry.key, c.API.APIKey)
		model := entry.model
		if entry.usesChatModel {
			model = firstNonempty(model, c.API.Model)
		}
		if model == "" && url == c.API.BaseURL && key == c.API.APIKey {
			continue
		}
		var providerID string
		for _, p := range c.Providers {
			if p.BaseURL == url && p.APIKey == key {
				providerID = p.ID
				break
			}
		}
		if providerID == "" {
			providerID = asciiLower(string(entry.task))
			c.addLegacyProvider(providerID, string(entry.task), url, key)
		}
		*c.taskAssignment(entry.task) = &ModelRef{ProviderID: providerID, Model: model}
	}
}

func (c *Config) addLegacyProvider(id, label, url, key string) {
	p := DefaultProviderConfig()
	p.ID, p.Label, p.BaseURL, p.APIKey = id, label, url, key
	c.Providers = append(c.Providers, p)
}

// migrateLegacyConfig copies the old default to the new one exactly once.
func migrateLegacyConfig(target, legacy string) error {
	if _, err := os.Stat(target); err == nil {
		return nil
	}
	data, err := os.ReadFile(legacy)
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("read legacy config %s: %w", legacy, err)
	}
	if err := os.MkdirAll(filepath.Dir(target), 0o700); err != nil {
		return fmt.Errorf("create config directory for %s: %w", target, err)
	}
	file, err := os.OpenFile(target, os.O_WRONLY|os.O_CREATE|os.O_EXCL, 0o600)
	if errors.Is(err, os.ErrExist) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("create migrated config %s: %w", target, err)
	}
	_, writeErr := file.Write(data)
	if err := errors.Join(writeErr, file.Close()); err != nil {
		return fmt.Errorf("copy legacy config to %s: %w", target, errors.Join(err, os.Remove(target)))
	}
	slog.Info("migrated legacy config.json to the new default location", "from", legacy, "to", target)
	return nil
}
