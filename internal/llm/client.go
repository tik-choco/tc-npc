// Package llm provides an OpenAI-compatible client for chat, embeddings, and audio.
package llm

import (
	"bytes"
	"context"
	"crypto/tls"
	"crypto/x509"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"log/slog"
	"math"
	"mime/multipart"
	"net/http"
	"net/textproto"
	"net/url"
	"os"
	"strings"
	"time"
)

const (
	defaultTimeout = 60 * time.Second
	ttsTimeout     = 15 * time.Second

	// ExtraCAEnv names a PEM bundle to trust in addition to the OS trust store.
	ExtraCAEnv = "TC_NPC_CA_BUNDLE"
	// InsecureTLSEnv disables certificate verification when set to 1, true, or yes.
	InsecureTLSEnv = "TC_NPC_INSECURE_TLS"
	// DefaultReasoningEffort is the explicit wire value for unconfigured chat requests.
	DefaultReasoningEffort = "none"
)

// StaticVoices returns a fresh list of the standard OpenAI voice names.
func StaticVoices() []string {
	return []string{"alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse"}
}

// RequestError reports request preparation, transport, or response body I/O failures.
type RequestError struct{ Err error }

func (e *RequestError) Error() string {
	// net/http includes the underlying TLS, DNS, or connection error in its message.
	message := "http request failed: " + e.Err.Error()
	var unknownAuthority x509.UnknownAuthorityError
	if errors.As(e.Err, &unknownAuthority) {
		message += " (hint: the endpoint's certificate is signed by a CA that isn't trusted. " +
			"Install its root CA in the OS trust store, or point " + ExtraCAEnv + " at a PEM bundle containing it.)"
	}
	return message
}

func (e *RequestError) Unwrap() error { return e.Err }

// APIError reports a non-2xx status and the response body, when readable.
type APIError struct {
	Status int
	Body   string
}

func (e *APIError) Error() string { return fmt.Sprintf("api error (%d): %s", e.Status, e.Body) }

// ParseError reports a response that could not be decoded as expected.
type ParseError struct{ Err error }

func (e *ParseError) Error() string { return "failed to parse response: " + e.Err.Error() }
func (e *ParseError) Unwrap() error { return e.Err }

// Client is an OpenAI-compatible API client safe for concurrent use.
type Client struct {
	http            *http.Client
	ttsHTTP         *http.Client
	baseURL         string
	apiKey          string
	reasoningEffort string
}

// New constructs an API client. It trims trailing slashes and adds /v1 unless
// the URL already ends in /v1 or selects a mistl room. An empty key omits bearer
// authentication. Chat requests default to DefaultReasoningEffort.
func New(baseURL, apiKey string) *Client {
	transport := newTransport()
	return &Client{
		http:            &http.Client{Transport: transport, Timeout: defaultTimeout, CheckRedirect: checkRedirect},
		ttsHTTP:         &http.Client{Transport: transport, Timeout: ttsTimeout, CheckRedirect: checkRedirect},
		baseURL:         normalizeBaseURL(baseURL),
		apiKey:          apiKey,
		reasoningEffort: DefaultReasoningEffort,
	}
}

// WithReasoningEffort returns a client copy with a different default for chat.
// An empty effort restores DefaultReasoningEffort.
func (c *Client) WithReasoningEffort(effort string) *Client {
	if effort == "" {
		effort = DefaultReasoningEffort
	}
	updated := *c
	updated.reasoningEffort = effort
	return &updated
}

func normalizeBaseURL(baseURL string) string {
	trimmed := strings.TrimRight(baseURL, "/")
	const rooms = "/rooms/"
	if i := strings.LastIndex(trimmed, rooms); i >= 0 {
		room := trimmed[i+len(rooms):]
		if room != "" && !strings.Contains(room, "/") {
			return trimmed
		}
	}
	if strings.HasSuffix(trimmed, "/v1") {
		return trimmed
	}
	return trimmed + "/v1"
}

func newTransport() http.RoundTripper {
	transport := http.DefaultTransport.(*http.Transport).Clone()
	// The Rust client is built without automatic compression support.
	transport.DisableCompression = true

	// The default net/http TLS verifier already uses the Windows OS trust store,
	// which serves the same purpose as Reqwest's rustls-tls-native-roots.
	// Keep the default verification path unless a TLS override was configured.
	var roots *x509.CertPool
	if path := os.Getenv(ExtraCAEnv); strings.TrimSpace(path) != "" {
		roots = extraRoots(path)
	}
	insecure := false
	switch strings.ToLower(strings.TrimSpace(os.Getenv(InsecureTLSEnv))) {
	case "1", "true", "yes":
		insecure = true
		slog.Warn("TLS certificate verification is disabled", "env", InsecureTLSEnv)
	}
	if roots != nil || insecure {
		if transport.TLSClientConfig == nil {
			transport.TLSClientConfig = &tls.Config{}
		}
		if roots != nil {
			transport.TLSClientConfig.RootCAs = roots
		}
		transport.TLSClientConfig.InsecureSkipVerify = insecure
	}
	return transport
}

func extraRoots(path string) *x509.CertPool {
	// An invalid extra bundle must not break endpoints trusted by the normal roots.
	bundle, err := os.ReadFile(path)
	if err != nil {
		slog.Warn("Cannot read extra CA bundle", "env", ExtraCAEnv, "path", path, "error", err)
		return nil
	}
	roots, err := x509.SystemCertPool()
	if err != nil {
		slog.Warn("Cannot load system CA pool for extra CA bundle", "env", ExtraCAEnv, "path", path, "error", err)
		return nil
	}
	if !roots.AppendCertsFromPEM(bundle) {
		slog.Warn("Extra CA bundle has no valid PEM certificates", "env", ExtraCAEnv, "path", path)
		return nil
	}
	return roots
}

func checkRedirect(req *http.Request, via []*http.Request) error {
	if len(via) > 10 {
		return errors.New("too many redirects")
	}
	previous := via[len(via)-1]
	// Go copies headers from the initial request. Once a redirect drops the
	// bearer key, keep it dropped for the rest of the chain, as Reqwest does.
	if previous.Header.Get("Authorization") == "" ||
		!strings.EqualFold(previous.URL.Hostname(), req.URL.Hostname()) || effectivePort(previous.URL) != effectivePort(req.URL) {
		req.Header.Del("Authorization")
	}
	return nil
}

func effectivePort(u *url.URL) string {
	if port := u.Port(); port != "" {
		return port
	}
	if u.Scheme == "https" {
		return "443"
	}
	return "80"
}

func (c *Client) send(ctx context.Context, client *http.Client, method, path, contentType string, body io.Reader) (*http.Response, error) {
	req, err := http.NewRequestWithContext(ctx, method, c.baseURL+path, body)
	if err != nil {
		return nil, &RequestError{Err: err}
	}
	req.Header.Set("Accept", "*/*")
	req.Header.Set("User-Agent", "") // Suppress the default Go User-Agent.
	if contentType != "" {
		req.Header.Set("Content-Type", contentType)
	}
	if c.apiKey != "" {
		req.Header.Set("Authorization", "Bearer "+c.apiKey)
	}
	resp, err := client.Do(req)
	if err != nil {
		return nil, &RequestError{Err: err}
	}
	if resp.StatusCode >= 200 && resp.StatusCode < 300 {
		return resp, nil
	}
	text := ""
	if data, readErr := readBody(resp); readErr == nil {
		text = strings.ToValidUTF8(string(data), "\uFFFD")
	}
	return nil, &APIError{Status: resp.StatusCode, Body: text}
}

func (c *Client) postJSON(ctx context.Context, client *http.Client, path string, payload any) (*http.Response, error) {
	data, err := json.Marshal(payload)
	if err != nil {
		return nil, &RequestError{Err: err}
	}
	return c.send(ctx, client, http.MethodPost, path, "application/json", bytes.NewReader(data))
}

func readBody(resp *http.Response) ([]byte, error) {
	defer resp.Body.Close()
	data, err := io.ReadAll(resp.Body)
	if err != nil {
		return nil, &RequestError{Err: err}
	}
	return data, nil
}

func readJSON(resp *http.Response, dst any) error {
	data, err := readBody(resp)
	if err != nil {
		return err
	}
	if err := json.Unmarshal(data, dst); err != nil {
		return &ParseError{Err: err}
	}
	return nil
}

func (c *Client) applyReasoningEffort(req *ChatRequest) {
	// Preserve a request's explicit choice and always send the parameter.
	if req.ReasoningEffort == nil {
		req.ReasoningEffort = &c.reasoningEffort
	}
}

func (c *Client) startChat(ctx context.Context, req ChatRequest, stream bool) (*http.Response, error) {
	req.Stream = &stream
	c.applyReasoningEffort(&req)
	return c.postJSON(ctx, c.http, "/chat/completions", req)
}

// Chat posts a non-streaming chat completion request.
func (c *Client) Chat(ctx context.Context, req ChatRequest) (ChatResponse, error) {
	var result ChatResponse
	resp, err := c.startChat(ctx, req, false)
	if err != nil {
		return result, err
	}
	err = readJSON(resp, &result)
	return result, err
}

// Embed returns embeddings in the order provided by the server.
func (c *Client) Embed(ctx context.Context, model string, inputs []string) ([][]float32, error) {
	if inputs == nil {
		inputs = []string{}
	}
	payload := struct {
		Model string   `json:"model"`
		Input []string `json:"input"`
	}{model, inputs}
	resp, err := c.postJSON(ctx, c.http, "/embeddings", payload)
	if err != nil {
		return nil, err
	}
	var result embeddingResponse
	if err := readJSON(resp, &result); err != nil {
		return nil, err
	}
	return result.values, nil
}

type embeddingResponse struct{ values [][]float32 }

func (r *embeddingResponse) UnmarshalJSON(data []byte) error {
	var items []embeddingItem
	if err := decodeObject(data, jsonField{"data", &items, required}); err != nil {
		return err
	}
	values := make([][]float32, len(items))
	for i, item := range items {
		values[i] = item.values
	}
	r.values = values
	return nil
}

type embeddingItem struct{ values []float32 }

func (i *embeddingItem) UnmarshalJSON(data []byte) error {
	var components []*float32
	if err := decodeObject(data, jsonField{"embedding", &components, required}); err != nil {
		return err
	}
	values := make([]float32, len(components))
	for j, component := range components {
		if component == nil {
			return fmt.Errorf("embedding component %d cannot be null", j)
		}
		values[j] = *component
	}
	i.values = values
	return nil
}

// Transcribe posts a WAV file as multipart form data and returns its text.
func (c *Client) Transcribe(ctx context.Context, model string, wav []byte) (string, error) {
	var body bytes.Buffer
	form := multipart.NewWriter(&body)
	if err := form.WriteField("model", model); err != nil {
		return "", &RequestError{Err: err}
	}
	part, err := form.CreatePart(textproto.MIMEHeader{
		"Content-Disposition": {`form-data; name="file"; filename="speech.wav"`},
		"Content-Type":        {"audio/wav"},
	})
	if err != nil {
		return "", &RequestError{Err: err}
	}
	if _, err := part.Write(wav); err != nil {
		return "", &RequestError{Err: err}
	}
	if err := form.Close(); err != nil {
		return "", &RequestError{Err: err}
	}
	resp, err := c.send(ctx, c.http, http.MethodPost, "/audio/transcriptions", form.FormDataContentType(), &body)
	if err != nil {
		return "", err
	}
	var result transcriptResponse
	if err := readJSON(resp, &result); err != nil {
		return "", err
	}
	return result.text, nil
}

type transcriptResponse struct{ text string }

func (r *transcriptResponse) UnmarshalJSON(data []byte) error {
	return decodeObject(data, jsonField{"text", &r.text, defaulted})
}

// Speak synthesizes text and returns the raw WAV response.
func (c *Client) Speak(ctx context.Context, model, voice, text string, speed float32) ([]byte, error) {
	payload := struct {
		Model          string   `json:"model"`
		Input          string   `json:"input"`
		Voice          string   `json:"voice"`
		ResponseFormat string   `json:"response_format"`
		Speed          *float32 `json:"speed"`
	}{model, text, voice, "wav", &speed}
	if math.IsNaN(float64(speed)) || math.IsInf(float64(speed), 0) {
		// Serde serializes non-finite floats as JSON null.
		payload.Speed = nil
	}
	resp, err := c.postJSON(ctx, c.ttsHTTP, "/audio/speech", payload)
	if err != nil {
		return nil, err
	}
	return readBody(resp)
}

// ListModels returns model IDs in the order provided by the server.
func (c *Client) ListModels(ctx context.Context) ([]string, error) {
	resp, err := c.send(ctx, c.http, http.MethodGet, "/models", "", nil)
	if err != nil {
		return nil, err
	}
	var result modelResponse
	if err := readJSON(resp, &result); err != nil {
		return nil, err
	}
	return result.ids, nil
}

type modelResponse struct{ ids []string }

func (r *modelResponse) UnmarshalJSON(data []byte) error {
	var items []modelItem
	if err := decodeObject(data, jsonField{"data", &items, defaulted}); err != nil {
		return err
	}
	ids := make([]string, len(items))
	for i, item := range items {
		ids[i] = item.id
	}
	r.ids = ids
	return nil
}

type modelItem struct{ id string }

func (i *modelItem) UnmarshalJSON(data []byte) error {
	return decodeObject(data, jsonField{"id", &i.id, required})
}

// ListVoices tries /audio/voices and then /voices before using StaticVoices.
func (c *Client) ListVoices(ctx context.Context) ([]string, error) {
	for _, path := range []string{"/audio/voices", "/voices"} {
		voices := c.tryListVoices(ctx, path)
		if err := ctx.Err(); err != nil {
			return nil, &RequestError{Err: err}
		}
		if len(voices) != 0 {
			return voices, nil
		}
	}
	return StaticVoices(), nil
}

func (c *Client) tryListVoices(ctx context.Context, path string) []string {
	resp, err := c.send(ctx, c.http, http.MethodGet, path, "", nil)
	if err != nil {
		return nil
	}
	var value any
	if err := readJSON(resp, &value); err != nil {
		return nil
	}
	return extractVoiceNames(value)
}

// Voice endpoints can use a bare array or a voices/data wrapper. Items can be
// strings or objects with a name or an ID.
func extractVoiceNames(value any) []string {
	items, ok := value.([]any)
	if !ok {
		wrapper, _ := value.(map[string]any)
		items, ok = wrapper["voices"].([]any)
		if !ok {
			items, _ = wrapper["data"].([]any)
		}
	}
	var voices []string
	for _, item := range items {
		name, ok := item.(string)
		if !ok {
			voice, _ := item.(map[string]any)
			name, ok = voice["name"].(string)
			if !ok {
				name, ok = voice["id"].(string)
			}
		}
		if ok {
			voices = append(voices, name)
		}
	}
	return voices
}
