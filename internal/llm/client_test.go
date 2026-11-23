package llm

import (
	"bytes"
	"context"
	"encoding/json"
	"encoding/pem"
	"errors"
	"fmt"
	"io"
	"math"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"
)

func TestNormalizeBaseURL(t *testing.T) {
	for _, tt := range []struct{ input, want string }{
		{"http://localhost:11434/v1", "http://localhost:11434/v1"},
		{"http://localhost:11434/v1/", "http://localhost:11434/v1"},
		{"https://api.openai.com", "https://api.openai.com/v1"},
		{"https://api.openai.com/", "https://api.openai.com/v1"},
		{"https://mist.example/rooms/room-a///", "https://mist.example/rooms/room-a"},
		{"https://mist.example/rooms/a/b", "https://mist.example/rooms/a/b/v1"},
	} {
		if got := normalizeBaseURL(tt.input); got != tt.want {
			t.Errorf("normalizeBaseURL(%q) = %q, want %q", tt.input, got, tt.want)
		}
	}
}

func TestApplyReasoningEffort(t *testing.T) {
	client := New("http://localhost:11434/v1", "")
	low, empty := "low", ""
	for _, tt := range []struct {
		name   string
		client *Client
		effort *string
		want   string
	}{
		{"default", client, nil, "none"},
		{"empty configuration", client.WithReasoningEffort(""), nil, "none"},
		{"high", client.WithReasoningEffort("high"), nil, "high"},
		{"request override", client.WithReasoningEffort("high"), &low, "low"},
		{"explicit empty override", client.WithReasoningEffort("high"), &empty, ""},
	} {
		t.Run(tt.name, func(t *testing.T) {
			req := NewChatRequest("m", nil)
			req.ReasoningEffort = tt.effort
			tt.client.applyReasoningEffort(&req)
			if req.ReasoningEffort == nil || *req.ReasoningEffort != tt.want {
				t.Errorf("reasoning effort = %v, want %q", req.ReasoningEffort, tt.want)
			}
		})
	}
}

func TestExtractVoiceNames(t *testing.T) {
	for _, tt := range []struct {
		wire string
		want []string
	}{
		{`["alloy","nova"]`, []string{"alloy", "nova"}},
		{`{"voices":["alloy"]}`, []string{"alloy"}},
		{`{"data":[{"id":"alloy"}]}`, []string{"alloy"}},
		{`[{"id":"ash","name":"nova"},7,{"name":null,"id":"echo"}]`, []string{"nova", "echo"}},
		{`{"voices":[],"data":["alloy"]}`, nil},
	} {
		var value any
		if err := json.Unmarshal([]byte(tt.wire), &value); err != nil {
			t.Fatal(err)
		}
		if got := extractVoiceNames(value); !reflect.DeepEqual(got, tt.want) {
			t.Errorf("extractVoiceNames(%s) = %v, want %v", tt.wire, got, tt.want)
		}
	}
}

func TestChat(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		checkRequest(t, r, http.MethodPost, "/v1/chat/completions", "Bearer api-key", "application/json")
		checkJSON(t, r.Body, `{
			"model":"test-model",
			"messages":[
				{"role":"system","content":"system"},
				{"role":"user","content":[{"type":"text","text":"see"},{"type":"image_url","image_url":{"url":"data:image/png;base64,QQ=="}}]},
				{"role":"assistant","content":"","tool_calls":[{"id":"call_0","type":"function","function":{"name":"lookup","arguments":"{}"}}]},
				{"role":"tool","content":"done","tool_call_id":"call_0"}
			],
			"tools":[{"type":"function","function":{"name":"lookup","description":"Find an entry","parameters":{"type":"object"}}}],
			"response_format":{"type":"json_object"},"max_tokens":0,"stream":false,"reasoning_effort":"low"
		}`)
		fmt.Fprint(w, `{"id":"r1","choices":[{"message":{"role":"assistant","content":null,"tool_calls":[{"id":"call_1","function":{"name":"lookup","arguments":"{\"q\":1}"}}]},"finish_reason":"tool_calls"},{"message":{"content":"second"}}]}`)
	}))
	defer server.Close()

	assistant := AssistantMessage("")
	assistant.ToolCalls = []ToolCall{{ID: "call_0", Type: "function", Function: FunctionCall{Name: "lookup", Arguments: "{}"}}}
	req := NewChatRequest("test-model", []ChatMessage{
		SystemMessage("system"),
		UserPartsMessage([]ContentPart{TextPart("see"), ImageDataURLPart("data:image/png;base64,QQ==")}),
		assistant,
		ToolResultMessage("call_0", "done"),
	})
	req.Tools = []Tool{FunctionTool("lookup", "Find an entry", json.RawMessage(`{"type":"object"}`))}
	format, low, max, stream := JSONObjectFormat(), "low", uint32(0), true
	req.ResponseFormat, req.ReasoningEffort, req.MaxTokens, req.Stream = &format, &low, &max, &stream
	result, err := New(server.URL+"/v1/", "api-key").WithReasoningEffort("high").Chat(t.Context(), req)
	if err != nil {
		t.Fatal(err)
	}
	if !*req.Stream || *req.ReasoningEffort != "low" {
		t.Error("Chat changed the caller's request")
	}
	if result.ID == nil || *result.ID != "r1" || result.Content() != nil || len(result.Choices) != 2 {
		t.Fatalf("unexpected chat response %+v", result)
	}
	first := result.Choices[0]
	if first.Index != 0 || first.FinishReason == nil || *first.FinishReason != "tool_calls" || first.Message.Role != "assistant" {
		t.Errorf("unexpected first choice %+v", first)
	}
	want := []ToolCall{{ID: "call_1", Type: "function", Function: FunctionCall{Name: "lookup", Arguments: `{"q":1}`}}}
	if got := result.ToolCalls(); !reflect.DeepEqual(got, want) {
		t.Errorf("tool calls = %+v, want %+v", got, want)
	}
}

func TestEmbed(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		checkRequest(t, r, http.MethodPost, "/rooms/abc/embeddings", "", "application/json")
		checkJSON(t, r.Body, `{"model":"embed-model","input":["one","two"]}`)
		fmt.Fprint(w, `{"data":[{"index":1,"embedding":[1.25,-0.5]},{"index":0,"embedding":[0.25,2]}]}`)
	}))
	defer server.Close()

	got, err := New(server.URL+"/rooms/abc/", "").Embed(t.Context(), "embed-model", []string{"one", "two"})
	want := [][]float32{{1.25, -0.5}, {0.25, 2}}
	if err != nil || !reflect.DeepEqual(got, want) {
		t.Errorf("Embed = %v, %v, want %v", got, err, want)
	}
}

func TestTranscribe(t *testing.T) {
	wav := []byte{'R', 'I', 'F', 'F', 0, 255, '\r', '\n'}
	for _, tt := range []struct{ response, want string }{
		{`{"text":"transcription","ignored":"value"}`, "transcription"},
		{`{}`, ""},
	} {
		t.Run(tt.response, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				checkRequest(t, r, http.MethodPost, "/v1/audio/transcriptions", "Bearer key", r.Header.Get("Content-Type"))
				reader, err := r.MultipartReader()
				if err != nil {
					t.Error(err)
					return
				}
				for _, field := range []struct {
					name, filename, mediaType string
					data                      []byte
				}{
					{"model", "", "", []byte("whisper")},
					{"file", "speech.wav", "audio/wav", wav},
				} {
					part, err := reader.NextPart()
					if err != nil {
						t.Error(err)
						return
					}
					if part.FormName() != field.name || part.FileName() != field.filename || part.Header.Get("Content-Type") != field.mediaType {
						t.Errorf("unexpected multipart part headers %v", part.Header)
					}
					if data, err := io.ReadAll(part); err != nil || !bytes.Equal(data, field.data) {
						t.Errorf("%s data = %q, %v, want %q", field.name, data, err, field.data)
					}
				}
				if _, err := reader.NextPart(); err != io.EOF {
					t.Errorf("multipart end = %v, want EOF", err)
				}
				fmt.Fprint(w, tt.response)
			}))
			defer server.Close()

			if got, err := New(server.URL, "key").Transcribe(t.Context(), "whisper", wav); err != nil || got != tt.want {
				t.Errorf("Transcribe = %q, %v, want %q", got, err, tt.want)
			}
		})
	}
}

func TestSpeak(t *testing.T) {
	audio := []byte{'R', 'I', 'F', 'F', 0, 255}
	for _, tt := range []struct {
		name, speedJSON string
		speed           float32
	}{
		{"normal", "1.25", 1.25},
		{"nonfinite", "null", float32(math.Inf(1))},
	} {
		t.Run(tt.name, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				checkRequest(t, r, http.MethodPost, "/v1/audio/speech", "Bearer key", "application/json")
				checkJSON(t, r.Body, `{"model":"tts","input":"say this","voice":"nova","response_format":"wav","speed":`+tt.speedJSON+`}`)
				w.Write(audio)
			}))
			defer server.Close()

			client := New(server.URL, "key")
			if client.http.Timeout != 60*time.Second || client.ttsHTTP.Timeout != 15*time.Second {
				t.Error("client timeouts differ from the Rust defaults")
			}
			if got, err := client.Speak(t.Context(), "tts", "nova", "say this", tt.speed); err != nil || !bytes.Equal(got, audio) {
				t.Errorf("Speak = %q, %v, want %q", got, err, audio)
			}
		})
	}
}

func TestListModels(t *testing.T) {
	for _, tt := range []struct {
		wire string
		want []string
	}{
		{`{"data":[{"id":"m2"},{"id":"m1"}]}`, []string{"m2", "m1"}},
		{`{}`, []string{}},
	} {
		server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			checkRequest(t, r, http.MethodGet, "/v1/models", "Bearer key", "")
			fmt.Fprint(w, tt.wire)
		}))
		got, err := New(server.URL, "key").ListModels(t.Context())
		server.Close()
		if err != nil || !reflect.DeepEqual(got, tt.want) {
			t.Errorf("ListModels for %s = %v, %v, want %v", tt.wire, got, err, tt.want)
		}
	}
}

func TestListVoices(t *testing.T) {
	for _, tt := range []struct {
		name, first, second string
		want                []string
		paths               []string
	}{
		{"audio voices", `["custom"]`, "", []string{"custom"}, []string{"/v1/audio/voices"}},
		{"second endpoint", `{"voices":[]}`, `{"data":[{"name":"custom"}]}`, []string{"custom"}, []string{"/v1/audio/voices", "/v1/voices"}},
		{"static fallback", "", `invalid JSON`, []string{"alloy", "ash", "ballad", "coral", "echo", "fable", "nova", "onyx", "sage", "shimmer", "verse"}, []string{"/v1/audio/voices", "/v1/voices"}},
	} {
		t.Run(tt.name, func(t *testing.T) {
			paths := make(chan string, 3)
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				paths <- r.URL.Path
				checkRequest(t, r, http.MethodGet, r.URL.Path, "Bearer key", "")
				wire := tt.second
				if r.URL.Path == "/v1/audio/voices" {
					wire = tt.first
				}
				if wire == "" {
					http.NotFound(w, r)
					return
				}
				fmt.Fprint(w, wire)
			}))
			got, err := New(server.URL, "key").ListVoices(t.Context())
			server.Close()
			close(paths)
			var visited []string
			for path := range paths {
				visited = append(visited, path)
			}
			if err != nil || !reflect.DeepEqual(got, tt.want) || !reflect.DeepEqual(visited, tt.paths) {
				t.Errorf("ListVoices = %v, %v, visited %v, want %v via %v", got, err, visited, tt.want, tt.paths)
			}
		})
	}
}

func TestClientErrors(t *testing.T) {
	for _, tt := range []struct {
		name, body string
		status     int
		truncated  bool
	}{
		{"api error", `{"error":{"message":"denied"}}`, http.StatusTooManyRequests, false},
		{"unreadable error body", "truncated", http.StatusBadGateway, true},
		{"unreadable success body", "truncated", http.StatusOK, true},
		{"invalid JSON", "{", http.StatusOK, false},
		{"missing message", `{"choices":[{}]}`, http.StatusOK, false},
		{"null choices", `{"choices":null}`, http.StatusOK, false},
	} {
		t.Run(tt.name, func(t *testing.T) {
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if tt.truncated {
					w.Header().Set("Content-Length", "100")
				}
				w.WriteHeader(tt.status)
				fmt.Fprint(w, tt.body)
			}))
			defer server.Close()
			_, err := New(server.URL, "").Chat(t.Context(), NewChatRequest("m", nil))
			switch {
			case tt.status != http.StatusOK:
				want := tt.body
				if tt.truncated {
					want = ""
				}
				var apiErr *APIError
				if !errors.As(err, &apiErr) || apiErr.Status != tt.status || apiErr.Body != want {
					t.Errorf("Chat error = %v, want APIError with status %d and body %q", err, tt.status, want)
				}
			case tt.truncated:
				var requestErr *RequestError
				if !errors.As(err, &requestErr) || !errors.Is(err, io.ErrUnexpectedEOF) {
					t.Errorf("Chat error = %v, want wrapped unexpected EOF", err)
				}
			default:
				var parseErr *ParseError
				if !errors.As(err, &parseErr) {
					t.Errorf("Chat error = %v, want ParseError", err)
				}
			}
		})
	}

	req := NewChatRequest("m", nil)
	req.Tools = []Tool{FunctionTool("f", "", json.RawMessage(`{`))}
	_, err := New("http://localhost", "").Chat(t.Context(), req)
	var requestErr *RequestError
	if !errors.As(err, &requestErr) {
		t.Errorf("invalid outgoing JSON = %v, want RequestError", err)
	}
}

func TestRedirectDropsBearerAcrossPorts(t *testing.T) {
	target := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		checkRequest(t, r, http.MethodGet, "/v1/models", "", "")
		fmt.Fprint(w, `{}`)
	}))
	defer target.Close()
	redirect := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		checkRequest(t, r, http.MethodGet, "/v1/models", "Bearer key", "")
		http.Redirect(w, r, target.URL+r.URL.Path, http.StatusTemporaryRedirect)
	}))
	defer redirect.Close()
	if _, err := New(redirect.URL, "key").ListModels(t.Context()); err != nil {
		t.Fatal(err)
	}
}

func TestTLSOverrides(t *testing.T) {
	t.Setenv(ExtraCAEnv, "")
	t.Setenv(InsecureTLSEnv, "")
	server := httptest.NewTLSServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		fmt.Fprint(w, `{}`)
	}))
	defer server.Close()
	_, err := New(server.URL, "").ListModels(t.Context())
	var requestErr *RequestError
	if !errors.As(err, &requestErr) || !strings.Contains(err.Error(), ExtraCAEnv) {
		t.Fatalf("untrusted TLS request = %v, want RequestError with CA hint", err)
	}

	bundle := filepath.Join(t.TempDir(), "ca.pem")
	cert := pem.EncodeToMemory(&pem.Block{Type: "CERTIFICATE", Bytes: server.Certificate().Raw})
	if err := os.WriteFile(bundle, cert, 0600); err != nil {
		t.Fatal(err)
	}
	t.Setenv(ExtraCAEnv, bundle)
	if _, err := New(server.URL, "").ListModels(t.Context()); err != nil {
		t.Errorf("extra CA bundle request = %v", err)
	}
	t.Setenv(ExtraCAEnv, bundle+".missing")
	t.Setenv(InsecureTLSEnv, " YeS ")
	if _, err := New(server.URL, "").ListModels(t.Context()); err != nil {
		t.Errorf("insecure TLS request = %v", err)
	}
}

func TestCanceledVoiceRequest(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	if _, err := New("http://localhost", "").ListVoices(ctx); !errors.Is(err, context.Canceled) {
		t.Errorf("ListVoices error = %v, want context.Canceled", err)
	}
}

func checkRequest(t *testing.T, r *http.Request, method, path, auth, contentType string) {
	t.Helper()
	if r.Method != method || r.URL.Path != path {
		t.Errorf("request = %s %s, want %s %s", r.Method, r.URL.Path, method, path)
	}
	for name, want := range map[string]string{
		"Authorization": auth, "Content-Type": contentType, "Accept": "*/*", "User-Agent": "", "Accept-Encoding": "",
	} {
		if got := r.Header.Get(name); got != want {
			t.Errorf("header %s = %q, want %q", name, got, want)
		}
	}
}

func checkJSON(t *testing.T, reader io.Reader, expected string) {
	t.Helper()
	data, err := io.ReadAll(reader)
	if err != nil {
		t.Error(err)
		return
	}
	var got, want any
	if err := json.Unmarshal(data, &got); err != nil {
		t.Errorf("invalid request JSON %q: %v", data, err)
		return
	}
	if err := json.Unmarshal([]byte(expected), &want); err != nil {
		t.Errorf("invalid expected JSON: %v", err)
		return
	}
	if !reflect.DeepEqual(got, want) {
		t.Errorf("JSON = %s, want %s", data, expected)
	}
}
