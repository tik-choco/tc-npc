package config

import "encoding/json"

// ModelRef identifies a provider and the model name to send to it.
type ModelRef struct {
	ProviderID string `json:"provider_id" required:"true"`
	Model      string `json:"model" required:"true"`
}

func (c *ModelRef) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, ModelRef{})
}

// ProviderConfig is an HTTP endpoint or Mist room with a model discovery cache.
type ProviderConfig struct {
	ID      string `json:"id"`
	Label   string `json:"label"`
	BaseURL string `json:"base_url"`
	// Secret. Use RedactedJSON when displaying the config.
	APIKey          string     `json:"api_key"`
	Enabled         bool       `json:"enabled"`
	Models          []string   `json:"models"`
	ModelsFetchedAt *string    `json:"models_fetched_at"`
	Provide         bool       `json:"provide"`
	Shared          []ModelRef `json:"shared"`
}

// DefaultProviderConfig returns the Rust defaults for ProviderConfig.
func DefaultProviderConfig() ProviderConfig {
	return ProviderConfig{
		Enabled: true,
		Models:  []string{},
		Shared:  []ModelRef{},
	}
}

func (c *ProviderConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, DefaultProviderConfig())
}

func (c ProviderConfig) MarshalJSON() ([]byte, error) {
	type wire ProviderConfig
	c.Models = nonNilSlice(c.Models)
	c.Shared = nonNilSlice(c.Shared)
	return json.Marshal(wire(c))
}

// PresetConfig is a legacy migration input.
type PresetConfig struct {
	ID              string `json:"id"`
	Label           string `json:"label"`
	ProviderID      string `json:"provider_id"`
	Model           string `json:"model"`
	ReasoningEffort string `json:"reasoning_effort"`
}

func (c *PresetConfig) UnmarshalJSON(data []byte) error {
	return decodeStruct(data, c, PresetConfig{})
}
