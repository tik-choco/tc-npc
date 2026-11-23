package server

import (
	"fmt"
	"log/slog"
	"net/http"
	"strings"

	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/llm"
)

type llmProbeRequest struct {
	BaseURL    string  `json:"baseUrl" required:"true"`
	APIKey     string  `json:"apiKey" required:"true"`
	Section    *string `json:"section"`
	ProviderID *string `json:"providerId"`
}

func (req *llmProbeRequest) UnmarshalJSON(data []byte) error {
	return decodeFields(data, req, true)
}

func resolveProbeAPIKey(providerID, section *string, key string, cfg *config.Config) string {
	if key != "***" {
		return key
	}
	if providerID != nil {
		if provider := cfg.Provider(*providerID); provider != nil {
			return provider.APIKey
		}
	}
	switch valueOr(section, "api") {
	case "api":
		return cfg.API.APIKey
	case "tts":
		return cfg.TTS.APIKey
	case "stt":
		return cfg.STT.APIKey
	default:
		return key
	}
}

func (s *appState) probeClient(w http.ResponseWriter, r *http.Request) *llm.Client {
	var req llmProbeRequest
	if !readJSONRequest(w, r, "invalid JSON: ", &req) {
		return nil
	}
	key := resolveProbeAPIKey(req.ProviderID, req.Section, req.APIKey, s.current.Load())
	return llm.New(req.BaseURL, key)
}

func (s *appState) apiLLMModels(w http.ResponseWriter, r *http.Request) {
	client := s.probeClient(w, r)
	if client == nil {
		return
	}
	models, err := client.ListModels(r.Context())
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	writeJSON(w, object{"models": models})
}

func (s *appState) apiLLMVoices(w http.ResponseWriter, r *http.Request) {
	client := s.probeClient(w, r)
	if client == nil {
		return
	}
	voices, err := client.ListVoices(r.Context())
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	writeJSON(w, object{"voices": voices})
}

type ocrPageRequest struct {
	ImageDataURL string `json:"imageDataUrl" required:"true"`
	PageNumber   uint32 `json:"pageNumber" required:"true"`
	TotalPages   uint32 `json:"totalPages" required:"true"`
}

func (req *ocrPageRequest) UnmarshalJSON(data []byte) error {
	return decodeFields(data, req, true)
}

func (s *appState) apiLLMOCR(w http.ResponseWriter, r *http.Request) {
	var req ocrPageRequest
	if !readJSONRequest(w, r, "", &req) {
		return
	}
	if strings.TrimSpace(req.ImageDataURL) == "" {
		writeError(w, http.StatusBadRequest, "imageDataUrl is empty")
		return
	}
	target, err := s.current.Load().Resolve(config.TaskVision)
	if err != nil {
		writeError(w, http.StatusPreconditionFailed, err.Error())
		return
	}
	if strings.TrimSpace(target.BaseURL) == "" {
		writeError(w, http.StatusPreconditionFailed, "the vision task has no LLM connection configured")
		return
	}
	client := llm.New(target.BaseURL, target.APIKey).WithReasoningEffort(target.ReasoningEffort)
	instruction := fmt.Sprintf("Extract all readable text from PDF page %d of %d. Return only the extracted text from this page. Do not summarize or add commentary.", req.PageNumber, req.TotalPages)
	msg := llm.UserPartsMessage([]llm.ContentPart{llm.TextPart(instruction), llm.ImageDataURLPart(req.ImageDataURL)})
	reply, err := client.Chat(r.Context(), llm.NewChatRequest(target.Model, []llm.ChatMessage{msg}))
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	writeJSON(w, object{"text": valueOr(reply.Content(), "")})
}

func (s *appState) apiLLMTranscribe(w http.ResponseWriter, r *http.Request) {
	body, ok := readUpload(w, r, maxAudioUploadBytes)
	if !ok {
		return
	}
	target, err := s.current.Load().Resolve(config.TaskSTT)
	if err != nil {
		writeError(w, http.StatusPreconditionFailed, err.Error())
		return
	}
	if strings.TrimSpace(target.BaseURL) == "" {
		writeError(w, http.StatusPreconditionFailed, "the speech-to-text task has no LLM connection configured")
		return
	}
	// The upstream upload name is speech.wav, the same as the voice
	// cascade. Keep the supplied name in diagnostics for rejected clips.
	slog.Debug("npc-server: transcribing an uploaded clip", "filename", r.PathValue("filename"), "bytes", len(body))
	text, err := llm.New(target.BaseURL, target.APIKey).Transcribe(r.Context(), target.Model, body)
	if err != nil {
		writeError(w, http.StatusBadGateway, err.Error())
		return
	}
	writeJSON(w, object{"text": text})
}
