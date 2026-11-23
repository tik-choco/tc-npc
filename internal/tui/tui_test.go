package tui

import (
	"io"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"sync"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
)

func TestResolveServerOverridesAndDiscovery(t *testing.T) {
	for _, tc := range []struct {
		opts Options
		base string
		ws   string
	}{
		{Options{ServerURL: "https://example.test/npc/", Addr: "ignored", Port: 80}, "https://example.test/npc", "wss://example.test/npc/ws"},
		{Options{Addr: "[::1]:7001", Port: 80}, "http://[::1]:7001", "ws://[::1]:7001/ws"},
		{Options{Port: 7000}, "http://127.0.0.1:7000", "ws://127.0.0.1:7000/ws"},
	} {
		base, ws, err := resolveServer(tc.opts)
		if err != nil || base != tc.base || ws != tc.ws {
			t.Errorf("resolveServer(%+v) = %q, %q, %v", tc.opts, base, ws, err)
		}
	}
	for _, opts := range []Options{{Addr: "localhost:7001"}, {ServerURL: "ftp://example.test"}, {ServerURL: "http://example.test?foo"}} {
		if _, _, err := resolveServer(opts); err == nil {
			t.Errorf("accepted invalid endpoint %+v", opts)
		}
	}
	home := t.TempDir()
	if got := discoverPort(home); got != defaultPort {
		t.Errorf("missing port file gave %d", got)
	}
	if err := os.Mkdir(filepath.Join(home, ".tc-npc"), 0o755); err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(home, ".tc-npc", "server-port.txt")
	for text, want := range map[string]uint16{" 51234\r\n": 51234, "bad": defaultPort, "65536": defaultPort} {
		if err := os.WriteFile(file, []byte(text), 0o644); err != nil {
			t.Fatal(err)
		}
		if got := discoverPort(home); got != want {
			t.Errorf("port %q gave %d, want %d", text, got, want)
		}
	}
}

func TestKeysAndOutgoingChat(t *testing.T) {
	m := &model{outgoing: &outgoingQueue{ready: make(chan struct{}, 1)}}
	m.handleKey(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune("q日本")})
	m.handleKey(tea.KeyMsg{Type: tea.KeyBackspace})
	if m.shouldQuit || m.input != "q日" {
		t.Fatalf("typing left quit=%v and input %q", m.shouldQuit, m.input)
	}
	m.handleKey(tea.KeyMsg{Type: tea.KeyEnter})
	if msg, ok := m.outgoing.pop(); !ok || msg.Text != "q日" || msg.Speaker != nil || m.input != "" {
		t.Fatalf("unexpected outgoing message: %+v, present=%v, input=%q", msg, ok, m.input)
	}
	m.handleKey(tea.KeyMsg{Type: tea.KeyPgUp})
	m.handleKey(tea.KeyMsg{Type: tea.KeyShiftTab})
	m.handleKey(tea.KeyMsg{Type: tea.KeyUp})
	if m.focus != focusLog || m.chatScroll != pageScrollLines || m.logScroll != 1 {
		t.Fatalf("unexpected focus/scroll: %v %d %d", m.focus, m.chatScroll, m.logScroll)
	}
	m.handleKey(tea.KeyMsg{Type: tea.KeyF2})
	m.handleKey(tea.KeyMsg{Type: tea.KeyEsc})
	if m.focus != focusInput || m.shouldQuit {
		t.Fatal("Esc in Config did not return to Main")
	}
	m.handleKey(tea.KeyMsg{Type: tea.KeyTab})
	m.handleKey(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune{'q'}})
	if !m.shouldQuit {
		t.Fatal("q outside input did not quit")
	}
	for _, key := range []tea.KeyType{tea.KeyEsc, tea.KeyCtrlC} {
		m.app = app{}
		m.handleKey(tea.KeyMsg{Type: key})
		if !m.shouldQuit {
			t.Errorf("%v in Main did not quit", key)
		}
	}
}

func TestConfigKeysFetchActivateAndRefresh(t *testing.T) {
	var mu sync.Mutex
	var requests []string
	active := "a"
	api := testAPI(t, func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		defer mu.Unlock()
		requests = append(requests, r.Method+" "+r.URL.Path)
		if r.Method == http.MethodPost {
			active = "b"
			_, _ = io.WriteString(w, `{"ok":true}`)
			return
		}
		if active == "a" {
			_, _ = io.WriteString(w, `[{"id":"a","name":"Alice","active":true},{"id":"b","name":"Bob","active":false}]`)
		} else {
			_, _ = io.WriteString(w, `[{"id":"a","name":"Alice","active":false},{"id":"b","name":"Bob","active":true}]`)
		}
	})
	m := &model{ctx: t.Context(), api: api}
	_, fetch := m.Update(tea.KeyMsg{Type: tea.KeyF2})
	if fetch == nil || m.focus != focusModules {
		t.Fatal("F2 did not enter Config and start a fetch")
	}
	m.Update(fetch())
	m.Update(tea.KeyMsg{Type: tea.KeyTab})
	m.Update(tea.KeyMsg{Type: tea.KeyDown})
	_, activate := m.Update(tea.KeyMsg{Type: tea.KeyEnter})
	if activate == nil || m.notice != "Bob に切り替えています..." {
		t.Fatalf("activation missing, notice=%q", m.notice)
	}
	m.Update(activate())
	if m.characterCursor != 1 || !m.characters[1].active || m.characters[0].active {
		t.Fatalf("after activation: cursor=%d, list=%+v", m.characterCursor, m.characters)
	}
	want := []string{"GET /api/characters", "POST /api/characters/b/activate", "GET /api/characters"}
	mu.Lock()
	if !slices.Equal(requests, want) {
		t.Errorf("REST calls = %v, want %v", requests, want)
	}
	mu.Unlock()
	m.Update(tea.KeyMsg{Type: tea.KeyEsc})
	_, refetch := m.Update(tea.KeyMsg{Type: tea.KeyF2})
	if refetch == nil {
		t.Fatal("Config re-entry did not schedule a refetch")
	}
}
