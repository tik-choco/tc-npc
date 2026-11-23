package server

import (
	"context"
	"encoding/json"
	"errors"
	"io"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"

	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
)

func TestRestoreMaskedSecrets(t *testing.T) {
	current := object{"api": object{"api_key": "DUMMY-current-key", "model": "old-model"}}
	for _, tc := range []struct{ incoming, want string }{{"***", "DUMMY-current-key"}, {"DUMMY-new-key", "DUMMY-new-key"}} {
		api := object{"api_key": tc.incoming, "model": "gpt-4o-mini"}
		restoreMaskedSecrets(object{"api": api}, current)
		if api["api_key"] != tc.want || api["model"] != "gpt-4o-mini" {
			t.Fatalf("restored config: %v", api)
		}
	}
}

func TestResolveProbeAPIKey(t *testing.T) {
	cfg := config.DefaultConfig()
	cfg.API.APIKey, cfg.TTS.APIKey, cfg.STT.APIKey = "DUMMY-api-key", "DUMMY-tts-key", "DUMMY-stt-key"
	cfg.Providers = []config.ProviderConfig{{ID: "p1", APIKey: "DUMMY-provider-key"}}
	for _, tc := range []struct {
		providerID, section *string
		input, want         string
	}{
		{nil, pointer("api"), "***", "DUMMY-api-key"},
		{nil, pointer("tts"), "***", "DUMMY-tts-key"},
		{nil, pointer("stt"), "***", "DUMMY-stt-key"},
		{nil, nil, "***", "DUMMY-api-key"},
		{pointer("p1"), pointer("api"), "***", "DUMMY-provider-key"},
		{pointer("missing"), pointer("tts"), "***", "DUMMY-tts-key"},
		{nil, pointer("api"), "DUMMY-typed-key", "DUMMY-typed-key"},
	} {
		if got := resolveProbeAPIKey(tc.providerID, tc.section, tc.input, &cfg); got != tc.want {
			t.Errorf("resolved key %q, want %q", got, tc.want)
		}
	}
}

func TestLLMProbeRequest(t *testing.T) {
	var req llmProbeRequest
	if err := json.Unmarshal([]byte("not json"), &req); err == nil {
		t.Fatal("accepted bad JSON")
	}
	req = decodeResponse[llmProbeRequest](t, []byte(`{"baseUrl":"http://localhost:11434","apiKey":"***","section":"tts"}`))
	if req.BaseURL != "http://localhost:11434" || req.APIKey != "***" || valueOr(req.Section, "") != "tts" || req.ProviderID != nil {
		t.Fatalf("section probe: %+v", req)
	}
	req = decodeResponse[llmProbeRequest](t, []byte(`{"baseUrl":"http://localhost:11434","apiKey":"***","providerId":"p1"}`))
	if valueOr(req.ProviderID, "") != "p1" || req.Section != nil {
		t.Fatalf("provider probe: %+v", req)
	}
}

type llmRequest struct {
	method, path, authorization string
	body                        object
}

func fakeLLM(t *testing.T) (*httptest.Server, <-chan llmRequest) {
	t.Helper()
	captured := make(chan llmRequest, 16)
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		var body object
		if strings.HasPrefix(r.Header.Get("Content-Type"), "application/json") {
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Errorf("decode upstream JSON: %v", err)
			}
		}
		captured <- llmRequest{r.Method, r.URL.EscapedPath(), r.Header.Get("Authorization"), body}
		w.Header().Set("Content-Type", "application/json")
		switch {
		case strings.HasSuffix(r.URL.Path, "/models"):
			_, _ = io.WriteString(w, `{"data":[{"id":"model"}]}`)
		case strings.HasSuffix(r.URL.Path, "/audio/voices"):
			_, _ = io.WriteString(w, `{"voices":["speaker"]}`)
		case strings.HasSuffix(r.URL.Path, "/audio/transcriptions"):
			_, _ = io.WriteString(w, `{"text":"heard"}`)
		case strings.HasSuffix(r.URL.Path, "/audio/speech"):
			w.Header().Set("Content-Type", "audio/wav")
			_, _ = io.WriteString(w, "wav")
		case body["stream"] == true:
			w.Header().Set("Content-Type", "text/event-stream")
			_, _ = io.WriteString(w, "data: {\"choices\":[{\"delta\":{\"content\":\"ok\"}}]}\n\ndata: [DONE]\n\n")
		default:
			_, _ = io.WriteString(w, `{"choices":[{"message":{"role":"assistant","content":"ocr"}}]}`)
		}
	}))
	t.Cleanup(srv.Close)
	return srv, captured
}

func TestRoomScopedModelsStreamVoiceAndOCR(t *testing.T) {
	s := testState(t)
	upstream, captured := fakeLLM(t)
	cfg := decodeResponse[config.Config](t, []byte(`{
		"providers":[{"id":"a","base_url":"mist-network://team/a %","api_key":"DUMMY-never-sent"},
		             {"id":"b","base_url":"mist-network://team-b"}],
		"default_ref":{"provider_id":"a","model":"model"},
		"talk":{"reasoning_effort":"xhigh"},
		"vision":{"model_ref":{"provider_id":"b","model":"model"},"reasoning_effort":"custom-effort"}
	}`))
	cfg.Mist.AIBaseURL = upstream.URL + "/custom/"
	s.current.Store(&cfg)
	target, err := cfg.Resolve(config.TaskTalk)
	if err != nil {
		t.Fatal(err)
	}
	client := llm.New(target.BaseURL, target.APIKey).WithReasoningEffort(target.ReasoningEffort)
	ctx := context.Background()
	if models, err := client.ListModels(ctx); err != nil || !reflect.DeepEqual(models, []string{"model"}) {
		t.Fatalf("models %v: %v", models, err)
	}
	stream, err := client.ChatStream(ctx, llm.NewChatRequest("model", nil))
	if err != nil {
		t.Fatal(err)
	}
	defer stream.Close()
	if next, err := stream.Next(); err != nil || next.Content != "ok" {
		t.Fatalf("first stream chunk: %+v, %v", next, err)
	}
	if _, err := stream.Next(); !errors.Is(err, io.EOF) {
		t.Fatalf("stream end: %v", err)
	}
	if wav, err := client.Speak(ctx, "model", "voice", "hello", 1); err != nil || string(wav) != "wav" {
		t.Fatalf("speech %q: %v", wav, err)
	}
	if heard, err := client.Transcribe(ctx, "model", []byte{0, 1}); err != nil || heard != "heard" {
		t.Fatalf("transcription %q: %v", heard, err)
	}
	h := s.routes()
	response := request(t, h, http.MethodPost, "/api/llm/ocr", `{"imageDataUrl":"data:image/png;base64,AA==","pageNumber":1,"totalPages":1}`, http.StatusOK)
	if response.Body.String() != `{"text":"ocr"}` {
		t.Fatalf("OCR: %s", response.Body)
	}
	for i, want := range []string{
		"/custom/rooms/team%2Fa%20%25/models", "/custom/rooms/team%2Fa%20%25/chat/completions",
		"/custom/rooms/team%2Fa%20%25/audio/speech", "/custom/rooms/team%2Fa%20%25/audio/transcriptions",
		"/custom/rooms/team-b/chat/completions",
	} {
		got := <-captured
		_, hasTemperature := got.body["temperature"]
		if got.path != want || got.authorization != "" || hasTemperature {
			t.Fatalf("upstream request %d: %+v, want %q without authorization or temperature", i, got, want)
		}
		if i == 1 && got.body["reasoning_effort"] != "xhigh" {
			t.Errorf("talk effort: %+v", got.body)
		}
		if i == 4 {
			content := got.body["messages"].([]any)[0].(map[string]any)["content"].([]any)
			if got.body["reasoning_effort"] != "custom-effort" || content[1].(map[string]any)["type"] != "image_url" {
				t.Errorf("OCR body: %+v", got.body)
			}
		}
	}
}

func TestRoomModelUsesRawModelAndNoSecrets(t *testing.T) {
	upstream, captured := fakeLLM(t)
	cfg := config.DefaultConfig()
	cfg.Providers = []config.ProviderConfig{{ID: "room", BaseURL: "mist-network://team", APIKey: "must-not-leak", Enabled: true}}
	cfg.DefaultRef = &config.ModelRef{ProviderID: "room", Model: "raw-model"}
	cfg.Mist.AIBaseURL = upstream.URL + "/v1/"
	cfg.Talk.ReasoningEffort = "high"
	target, err := cfg.Resolve(config.TaskTalk)
	if err != nil {
		t.Fatal(err)
	}
	client := llm.New(target.BaseURL, target.APIKey).WithReasoningEffort(target.ReasoningEffort)
	if _, err := client.Chat(context.Background(), llm.NewChatRequest(target.Model, []llm.ChatMessage{})); err != nil {
		t.Fatal(err)
	}
	got := <-captured
	_, hasTemperature := got.body["temperature"]
	if got.method != http.MethodPost || got.path != "/v1/rooms/team/chat/completions" || got.authorization != "" ||
		got.body["model"] != "raw-model" || got.body["reasoning_effort"] != "high" || hasTemperature {
		t.Fatalf("room request: %+v", got)
	}
}

func TestLLMProbeAndTranscriptionRoutes(t *testing.T) {
	s := testState(t)
	upstream, captured := fakeLLM(t)
	cfg := s.current.Load()
	cfg.Providers = []config.ProviderConfig{{ID: "p", BaseURL: upstream.URL, APIKey: "DUMMY-upstream-key", Enabled: true}}
	cfg.DefaultRef = &config.ModelRef{ProviderID: "p", Model: "model"}
	h := s.routes()
	probe := jsonText(t, object{"baseUrl": upstream.URL, "apiKey": "***", "providerId": "p"})
	for _, tc := range []struct{ path, body, want string }{
		{"/api/llm/models", probe, `{"models":["model"]}`},
		{"/api/llm/voices", probe, `{"voices":["speaker"]}`},
		{"/api/llm/transcribe/recording.wav", "clip", `{"text":"heard"}`},
	} {
		got := request(t, h, http.MethodPost, tc.path, tc.body, http.StatusOK)
		if got.Body.String() != tc.want || (<-captured).authorization != "Bearer DUMMY-upstream-key" {
			t.Fatalf("%s: %s", tc.path, got.Body)
		}
	}
}
