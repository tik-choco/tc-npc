package server

import (
	"net/http"
	"os"
	"strings"
	"testing"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
)

func TestConfigRoundTripRedactsRestoresAndPublishes(t *testing.T) {
	s := testState(t)
	cfg := s.current.Load()
	cfg.API.APIKey = "DUMMY-legacy-key"
	cfg.Providers = []config.ProviderConfig{
		{ID: "one", APIKey: "DUMMY-first-key", BaseURL: "http://one/v1", Enabled: true},
		{ID: "two", APIKey: "DUMMY-second-key", BaseURL: "http://two/v1", Enabled: true},
	}
	h := s.routes()
	sub := s.mc.Bus.Subscribe()
	defer sub.Close()
	w := request(t, h, http.MethodGet, "/api/config", "", http.StatusOK)
	doc := decodeResponse[object](t, w.Body.Bytes())
	providers := doc["providers"].([]any)
	if strings.Contains(w.Body.String(), "DUMMY-") || doc["api"].(map[string]any)["api_key"] != "***" ||
		providers[0].(map[string]any)["api_key"] != "***" || providers[1].(map[string]any)["api_key"] != "***" {
		t.Fatal("config exposed a key or did not emit the mask")
	}

	// Reordering masked providers preserves their credentials by ID.
	providers[0], providers[1] = providers[1], providers[0]
	request(t, h, http.MethodPut, "/api/config", jsonText(t, doc), http.StatusOK)
	msg := receiveBus(t, sub)
	saved := decodeResponse[config.Config](t, msg.Payload)
	if msg.Topic != bus.TopicConfig || msg.Type != bus.MsgConfigUpdated || saved.API.APIKey != "DUMMY-legacy-key" ||
		saved.Providers[0].ID != "two" || saved.Providers[0].APIKey != "DUMMY-second-key" ||
		saved.Providers[1].ID != "one" || saved.Providers[1].APIKey != "DUMMY-first-key" {
		t.Fatal("config update failed to carry the full config with restored keys")
	}
	raw, err := os.ReadFile(s.mc.ConfigPath)
	if err != nil {
		t.Fatal(err)
	}
	if disk := decodeResponse[config.Config](t, raw); disk.Providers[0].APIKey != saved.Providers[0].APIKey {
		t.Fatal("saved config differs from published config")
	}

	request(t, h, http.MethodPut, "/api/config", `{"scheduler":42}`, http.StatusBadRequest)
	request(t, h, http.MethodPut, "/api/config", `{"scheduler"`, http.StatusBadRequest)
	w = request(t, h, http.MethodGet, "/api/config", "", http.StatusOK)
	if !strings.Contains(w.Body.String(), "\"id\":\"two\"") || strings.Contains(w.Body.String(), "DUMMY-second-key") {
		t.Fatal("GET config failed to return the latest redacted snapshot")
	}
}
