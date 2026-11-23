package config

import (
	"bytes"
	"errors"
	"os"
	"path/filepath"
	"slices"
	"unicode/utf8"

	"github.com/joho/godotenv"
)

func loadDotEnv() {
	dir, err := os.Getwd()
	if err != nil {
		return
	}
	for {
		path := filepath.Join(dir, ".env")
		info, err := os.Stat(path)
		if err == nil && info.Mode().IsRegular() {
			data, err := os.ReadFile(path)
			if err != nil {
				return
			}
			// Retain process values, including values set to an empty string.
			// Keep entries read before a malformed line, as with dotenvy.
			values, _ := godotenv.UnmarshalBytes(bytes.TrimPrefix(data, []byte{0xef, 0xbb, 0xbf}))
			for key, value := range values {
				if _, exists := os.LookupEnv(key); !exists {
					_ = os.Setenv(key, value)
				}
			}
			return
		}
		parent := filepath.Dir(dir)
		if (err != nil && !errors.Is(err, os.ErrNotExist)) || parent == dir {
			return
		}
		dir = parent
	}
}

type lookupEnv func(string) (string, bool)

func (c *Config) applyEnvOverrides(lookup lookupEnv) {
	// Rust env::var only returns valid Unicode values. Presence alone still
	// triggers the isolated TTS/STT provider override below.
	env := func(name string) (string, bool) {
		value, ok := lookup(name)
		return value, ok && utf8.ValidString(value)
	}
	for _, field := range []struct {
		name  string
		value *string
	}{
		{"OPENAI_API_KEY", &c.API.APIKey},
		{"OPENAI_API_BASE_URL", &c.API.BaseURL},
		{"OPENAI_MODEL", &c.API.Model},
		{"OPENAI_EMBEDDING_MODEL", &c.API.EmbeddingModel},
		{"OPENAI_REASONING_EFFORT", &c.API.ReasoningEffort},
		{"TTS_BASE_URL", &c.TTS.BaseURL},
		{"TTS_API_KEY", &c.TTS.APIKey},
		{"STT_BASE_URL", &c.STT.BaseURL},
		{"STT_API_KEY", &c.STT.APIKey},
	} {
		if value, ok := env(field.name); ok {
			*field.value = value
		}
	}
	if model, ok := env("OPENAI_MODEL"); ok {
		if c.DefaultRef == nil {
			c.DefaultRef = &ModelRef{ProviderID: "default"}
		}
		c.DefaultRef.Model = model
	}
	if model, ok := env("OPENAI_EMBEDDING_MODEL"); ok {
		if c.Memory.EmbeddingRef == nil {
			c.Memory.EmbeddingRef = &ModelRef{}
			if c.DefaultRef != nil {
				*c.Memory.EmbeddingRef = *c.DefaultRef
			}
		}
		c.Memory.EmbeddingRef.Model = model
	}
	if c.DefaultRef != nil {
		c.overrideProviderEnv(c.DefaultRef.ProviderID, "OPENAI_API_BASE_URL", "OPENAI_API_KEY", env)
	}
	for _, voice := range []struct {
		task     LlmTask
		url, key string
	}{
		{TaskTTS, "TTS_BASE_URL", "TTS_API_KEY"},
		{TaskSTT, "STT_BASE_URL", "STT_API_KEY"},
	} {
		_, hasURL := lookup(voice.url)
		_, hasKey := lookup(voice.key)
		if !hasURL && !hasKey {
			continue
		}
		source := c.TaskRef(voice.task)
		if source == nil {
			source = c.DefaultRef
		}
		if source == nil {
			continue
		}
		provider := c.Provider(source.ProviderID)
		if provider == nil {
			continue
		}
		// Voice endpoint overrides get their own provider so they leave chat intact.
		id := "env-" + asciiLower(string(voice.task))
		for c.Provider(id) != nil {
			id += "-"
		}
		copy := *provider
		copy.ID = id
		copy.Models = slices.Clone(provider.Models)
		copy.Shared = slices.Clone(provider.Shared)
		if provider.ModelsFetchedAt != nil {
			stamp := *provider.ModelsFetchedAt
			copy.ModelsFetchedAt = &stamp
		}
		c.Providers = append(c.Providers, copy)
		c.overrideProviderEnv(id, voice.url, voice.key, env)
		*c.taskAssignment(voice.task) = &ModelRef{ProviderID: id, Model: source.Model}
	}
}

func (c *Config) overrideProviderEnv(id, url, key string, env lookupEnv) {
	provider := c.Provider(id)
	if provider == nil {
		return
	}
	if _, room := provider.Room(); room {
		if value, ok := env(url); ok {
			c.Mist.AIBaseURL = value
		}
		return
	}
	if value, ok := env(url); ok {
		provider.BaseURL = value
	}
	if value, ok := env(key); ok {
		provider.APIKey = value
	}
}
