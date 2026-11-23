package tui

import (
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
	"time"
)

func testAPI(t *testing.T, handler http.HandlerFunc) apiClient {
	t.Helper()
	server := httptest.NewServer(handler)
	t.Cleanup(server.Close)
	return apiClient{http: server.Client(), base: server.URL}
}

func TestListCharactersAllowsMissingAvatar(t *testing.T) {
	api := testAPI(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodGet || r.URL.Path != "/api/characters" {
			t.Errorf("unexpected request: %s %s", r.Method, r.URL)
		}
		_, _ = io.WriteString(w, `[{"id":"c1","name":"Alice","active":true,"avatar":{"kind":"vrm","file":"a.vrm"}},{"id":"c2","name":"Bob","active":false}]`)
	})
	characters, err := api.listCharacters(t.Context())
	if err != nil {
		t.Fatal(err)
	}
	if len(characters) != 2 {
		t.Fatalf("got %d characters, want two", len(characters))
	}
	alice, bob := characters[0], characters[1]
	if alice.ID != "c1" || !alice.Active || alice.Avatar == nil || *alice.Avatar != (avatarRef{"vrm", "a.vrm"}) ||
		bob.ID != "c2" || bob.Active || bob.Avatar != nil {
		t.Fatalf("unexpected summaries: %+v", characters)
	}
}

func TestActivateCharacterPostsEncodedPath(t *testing.T) {
	api := testAPI(t, func(w http.ResponseWriter, r *http.Request) {
		if r.Method != http.MethodPost || r.RequestURI != "/api/characters/odd%20id%2F%E6%97%A5%2B/activate" {
			t.Errorf("unexpected request: %s %s", r.Method, r.RequestURI)
		}
		_, _ = io.WriteString(w, `{"ok":true,"note":"persona and avatar switched immediately; no restart required"}`)
	})
	raw, err := api.activateCharacter(t.Context(), "odd id/日+")
	if err != nil {
		t.Fatal(err)
	}
	var result map[string]any
	if err = json.Unmarshal(raw, &result); err != nil || result["ok"] != true || result["note"] == nil {
		t.Fatalf("unexpected response: %s, error %v", raw, err)
	}
}

func TestREST404SurfacesServerMessage(t *testing.T) {
	api := testAPI(t, func(w http.ResponseWriter, _ *http.Request) {
		w.WriteHeader(http.StatusNotFound)
		_, _ = io.WriteString(w, `{"error":"character not found: missing"}`)
	})
	_, err := api.activateCharacter(t.Context(), "missing")
	if err == nil || !strings.Contains(err.Error(), "404") || !strings.Contains(err.Error(), "character not found: missing") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestNonJSONResponseIsParseError(t *testing.T) {
	api := testAPI(t, func(w http.ResponseWriter, _ *http.Request) {
		_, _ = io.WriteString(w, "this is not json")
	})
	_, err := api.listCharacters(t.Context())
	if err == nil || !strings.Contains(err.Error(), "parsing JSON response body") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestIncompleteHeadersAreAnError(t *testing.T) {
	api := testAPI(t, func(w http.ResponseWriter, _ *http.Request) {
		conn, buf, err := w.(http.Hijacker).Hijack()
		if err != nil {
			t.Error(err)
			return
		}
		defer conn.Close()
		_, _ = buf.WriteString("HTTP/1.1 200")
		_ = buf.Flush()
	})
	_, err := api.listCharacters(t.Context())
	if err == nil || !strings.Contains(err.Error(), "malformed HTTP response") {
		t.Fatalf("unexpected error: %v", err)
	}
}

func TestRESTRequestTimesOutWithoutResponse(t *testing.T) {
	api := testAPI(t, func(_ http.ResponseWriter, r *http.Request) {
		<-r.Context().Done()
	})
	_, err := api.requestJSON(t.Context(), http.MethodGet, "/api/config", 150*time.Millisecond)
	if err == nil || !strings.Contains(err.Error(), "did not respond") || !strings.Contains(err.Error(), "150ms") {
		t.Fatalf("unexpected error: %v", err)
	}
}
