package server

import (
	"io/fs"
	"net/http"
	"net/http/httptest"
	"path"
	"strings"
	"testing"

	"github.com/tik-choco/tc-npc/web"
)

func TestStaticAssetsAndSPA(t *testing.T) {
	h := testState(t).routes()
	index, err := fs.ReadFile(web.Dist, "dist/index.html")
	if err != nil {
		t.Fatal(err)
	}
	for _, url := range []string{"/", "/ui/deep/link", "/apis", "/wss", "/ui//nested"} {
		w := request(t, h, http.MethodGet, url, "", http.StatusOK)
		if w.Body.String() != string(index) || w.Header().Get("Content-Type") != "text/html" || w.Header().Get("Cache-Control") != "" {
			t.Fatalf("%s served the wrong static response: %v", url, w.Header())
		}
	}
	for _, url := range []string{"/api", "/api/missing", "/ws/missing"} {
		if w := request(t, h, http.MethodGet, url, "", http.StatusNotFound); w.Body.Len() != 0 {
			t.Fatalf("404 body for %s: %q", url, w.Body)
		}
	}

	mediaTypes := map[string]string{".html": "text/html", ".css": "text/css", ".js": "text/javascript", ".mjs": "application/javascript"}
	err = fs.WalkDir(web.Dist, "dist", func(name string, entry fs.DirEntry, err error) error {
		if err != nil || entry.IsDir() {
			return err
		}
		wantType, ok := mediaTypes[path.Ext(name)]
		if !ok {
			return nil
		}
		data, err := fs.ReadFile(web.Dist, name)
		if err != nil {
			return err
		}
		w := request(t, h, http.MethodGet, "/"+strings.TrimPrefix(name, "dist/"), "", http.StatusOK)
		if w.Body.String() != string(data) || w.Header().Get("Content-Type") != wantType {
			t.Errorf("%s: incorrect bytes or MIME type %q", name, w.Header().Get("Content-Type"))
		}
		return nil
	})
	if err != nil {
		t.Fatal(err)
	}
}

func TestMethodAndOriginHandling(t *testing.T) {
	h := testState(t).routes()
	for _, method := range []string{http.MethodGet, http.MethodPost, http.MethodDelete, http.MethodOptions} {
		r := httptest.NewRequest(method, "/api/vrm/default", nil)
		r.Header.Set("Origin", "https://example.test")
		r.Header.Set("Access-Control-Request-Method", "PUT")
		w := recordResponse(t, h, r, http.StatusMethodNotAllowed)
		if w.Header().Get("Allow") != "PUT" || w.Body.Len() != 0 {
			t.Fatalf("default avatar method rejection: %v %q", w.Header(), w.Body)
		}
		if w.Header().Get("Access-Control-Allow-Origin") != "" {
			t.Fatal("HTTP response unexpectedly added CORS headers")
		}
	}
	// Literal matching happens before path-parameter decoding.
	request(t, h, http.MethodPost, "/api/vrm/%64efault", "data", http.StatusBadRequest)
	request(t, h, http.MethodGet, "/api/people/..", "", http.StatusBadRequest)
	r := httptest.NewRequest(http.MethodGet, "/healthz", nil)
	r.Header.Set("Origin", "https://example.test")
	w := recordResponse(t, h, r, http.StatusOK)
	if w.Body.String() != "{\"ok\":true}" || w.Header().Get("Access-Control-Allow-Origin") != "" {
		t.Fatalf("health response: %v %q", w.Header(), w.Body)
	}
	w = request(t, h, http.MethodGet, "/ws", "", http.StatusBadRequest)
	if w.Header().Get("Content-Type") != "text/plain; charset=utf-8" ||
		w.Body.String() != "Connection header did not include 'upgrade'" {
		t.Fatalf("WebSocket extractor rejection: %v %q", w.Header(), w.Body)
	}
}
