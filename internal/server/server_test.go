package server

import (
	"context"
	"encoding/json"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/module"
)

func testState(t *testing.T) *appState {
	t.Helper()
	ctx, cancel := context.WithCancel(context.Background())
	t.Cleanup(cancel)
	dir := t.TempDir()
	cfg := config.DefaultConfig()
	return newAppState(ctx, module.Ctx{
		Bus: bus.New(), Config: &cfg, ConfigPath: filepath.Join(dir, "config.json"), DataDir: dir,
	}, "127.0.0.1:0")
}

func putFile(t *testing.T, path string, contents []byte) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, contents, 0o600); err != nil {
		t.Fatal(err)
	}
}

func pointer[T any](v T) *T { return &v }

func jsonText(t *testing.T, value any) string {
	t.Helper()
	raw, err := json.Marshal(value)
	if err != nil {
		t.Fatal(err)
	}
	return string(raw)
}

func decodeResponse[T any](t *testing.T, raw []byte) T {
	t.Helper()
	var out T
	if err := json.Unmarshal(raw, &out); err != nil {
		t.Fatalf("decode %s: %v", raw, err)
	}
	return out
}

func request(t *testing.T, h http.Handler, method, path, body string, status int) *httptest.ResponseRecorder {
	t.Helper()
	r := httptest.NewRequest(method, path, strings.NewReader(body))
	return recordResponse(t, h, r, status)
}

func recordResponse(t *testing.T, h http.Handler, r *http.Request, status int) *httptest.ResponseRecorder {
	t.Helper()
	w := httptest.NewRecorder()
	h.ServeHTTP(w, r)
	if w.Code != status {
		t.Fatalf("%s %s: status %d, want %d, body %q", r.Method, r.URL, w.Code, status, w.Body)
	}
	return w
}

func receiveBus(t *testing.T, sub *bus.Subscription) bus.Message {
	t.Helper()
	select {
	case msg := <-sub.C:
		return msg
	case <-time.After(5 * time.Second):
		t.Fatal("timed out waiting for bus message")
		return bus.Message{}
	}
}

func waitUntil(t *testing.T, ready func() bool) {
	t.Helper()
	deadline := time.Now().Add(5 * time.Second)
	for !ready() {
		if time.Now().After(deadline) {
			t.Fatal("condition was not met")
		}
		time.Sleep(5 * time.Millisecond)
	}
}

func TestModulePublishesAndCleansUpBoundPort(t *testing.T) {
	s := testState(t)
	s.mc.Config.Server.AutoOpen = false
	s.mc.Config.Server.Addr = "127.0.0.1:0"
	m, err := New(s.mc)
	if err != nil || m.Name() != "npc-server" {
		t.Fatalf("module %v: %v", m, err)
	}
	ctx, cancel := context.WithCancel(s.ctx)
	defer cancel()
	done := make(chan error, 1)
	go func() { done <- m.Run(ctx, s.mc) }()
	portPath := filepath.Join(s.mc.DataDir, serverPortFileName)
	var port string
	waitUntil(t, func() bool {
		data, err := os.ReadFile(portPath)
		port = string(data)
		return err == nil && port != ""
	})
	n, err := strconv.Atoi(port)
	if err != nil || n <= 0 {
		t.Fatalf("bound port file: %q", port)
	}
	client := &http.Client{Timeout: 5 * time.Second}
	resp, err := client.Get("http://127.0.0.1:" + port + "/api/state")
	if err != nil {
		t.Fatal(err)
	}
	body, err := io.ReadAll(resp.Body)
	_ = resp.Body.Close()
	if err != nil || resp.StatusCode != http.StatusOK ||
		decodeResponse[object](t, body)["addr"] != "127.0.0.1:"+port {
		t.Fatalf("state: status %d, %s, %v", resp.StatusCode, body, err)
	}
	cancel()
	select {
	case err := <-done:
		if err != nil {
			t.Fatal(err)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("server did not shut down")
	}
	if _, err := os.Stat(portPath); !os.IsNotExist(err) {
		t.Fatalf("port file should be gone: %v", err)
	}
}

func TestBindWithRetry(t *testing.T) {
	ctx := context.Background()
	busy, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer busy.Close()
	got, err := bindWithRetry(ctx, busy.Addr().String())
	if err != nil {
		t.Fatal(err)
	}
	defer got.Close()
	base, actual := busy.Addr().(*net.TCPAddr).Port, got.Addr().(*net.TCPAddr).Port
	if actual <= base || actual >= base+maxBindAttempts {
		t.Fatalf("bound %d after starting at %d", actual, base)
	}
	if got, err := bindWithRetry(ctx, "localhost:0"); err == nil {
		got.Close()
		t.Fatal("expected socket-address parse error")
	}
}
