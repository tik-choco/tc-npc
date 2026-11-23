// Package tui runs the tc-npc terminal client over HTTP and WebSocket.
package tui

import (
	"context"
	"fmt"
	"net"
	"net/http"
	"net/netip"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
	"unicode"
	"unicode/utf8"

	tea "github.com/charmbracelet/bubbletea"
)

const defaultPort = 47950

// Options selects the server for Run. Its zero value discovers the port
// published by the local server, falling back to 127.0.0.1:47950.
type Options struct {
	// ServerURL is an optional base URL, with an optional path prefix.
	// It takes precedence over Addr and Port. Use http/https or ws/wss.
	ServerURL string
	// Addr is an optional IP:port, matching the Rust CLI's --addr flag.
	// It takes precedence over Port and local discovery.
	Addr string
	// Port overrides local discovery on 127.0.0.1 when nonzero.
	Port uint16
}

// Run attaches to a running tc-npc server and owns the terminal's raw mode
// and alternate screen until the user quits or ctx is canceled. Quit returns
// nil. Cancellation returns ctx.Err(). Connection failures appear in the
// TUI while it retries.
func Run(ctx context.Context, opts Options) error {
	if err := ctx.Err(); err != nil {
		return err
	}
	base, ws, err := resolveServer(opts)
	if err != nil {
		return err
	}

	ctx, cancel := context.WithCancel(ctx)
	protocols := new(http.Protocols)
	protocols.SetHTTP1(true) // WebSocket uses HTTP/1.1 Upgrade, including over TLS.
	httpClient := &http.Client{
		Transport: &http.Transport{IdleConnTimeout: 90 * time.Second, Protocols: protocols},
		CheckRedirect: func(*http.Request, []*http.Request) error {
			return http.ErrUseLastResponse
		},
	}
	events := make(chan any, 16)
	outgoing := &outgoingQueue{ready: make(chan struct{}, 1)}
	done := make(chan struct{})
	go func() {
		defer close(done)
		reconnectLoop(ctx, ws, httpClient, events, outgoing)
	}()
	defer func() {
		cancel()
		<-done
		httpClient.CloseIdleConnections()
	}()

	m := &model{ctx: ctx, api: apiClient{http: httpClient, base: base}, events: events, outgoing: outgoing}
	program := tea.NewProgram(m, tea.WithContext(ctx), tea.WithAltScreen())
	stopResize := watchTerminalResize(ctx, program)
	defer stopResize()
	_, err = program.Run()
	if ctxErr := ctx.Err(); ctxErr != nil {
		return ctxErr
	}
	if err != nil {
		return fmt.Errorf("running tc-npc terminal: %w", err)
	}
	return nil
}

func resolveServer(opts Options) (base, ws string, err error) {
	if opts.ServerURL != "" {
		u, err := url.Parse(opts.ServerURL)
		if err != nil {
			return "", "", fmt.Errorf("parsing tc-npc server URL: %w", err)
		}
		if u.Hostname() == "" || u.User != nil || u.ForceQuery || u.RawQuery != "" || u.Fragment != "" {
			return "", "", fmt.Errorf("invalid tc-npc server base URL %q", opts.ServerURL)
		}
		if port := u.Port(); port != "" {
			if _, err := strconv.ParseUint(port, 10, 16); err != nil {
				return "", "", fmt.Errorf("invalid tc-npc server URL port: %w", err)
			}
		}
		switch u.Scheme {
		case "http", "ws":
			u.Scheme = "http"
			base = strings.TrimRight(u.String(), "/")
			u.Scheme = "ws"
		case "https", "wss":
			u.Scheme = "https"
			base = strings.TrimRight(u.String(), "/")
			u.Scheme = "wss"
		default:
			return "", "", fmt.Errorf("unsupported tc-npc server URL scheme %q", u.Scheme)
		}
		return base, strings.TrimRight(u.String(), "/") + "/ws", nil
	}

	addr := opts.Addr
	if addr != "" {
		parsed, err := netip.ParseAddrPort(addr)
		if err != nil {
			return "", "", fmt.Errorf("parsing tc-npc server address: %w", err)
		}
		addr = parsed.String()
	} else {
		port := opts.Port
		if port == 0 {
			port = discoverPort(userHomeDir())
		}
		addr = net.JoinHostPort("127.0.0.1", strconv.Itoa(int(port)))
	}
	u := url.URL{Scheme: "http", Host: addr}
	base = u.String()
	u.Scheme, u.Path = "ws", "/ws"
	return base, u.String(), nil
}

// The configured port is only a starting point. A server that falls forward
// to a free port publishes the actual bound port in this file.
func discoverPort(home string) uint16 {
	dataDir := ""
	if home != "" {
		dataDir = filepath.Join(home, ".tc-npc")
	}
	// Same override as config.DataDirEnv (this package deliberately does not
	// import internal/config).
	if dir := os.Getenv("TC_NPC_DATA_DIR"); dir != "" {
		dataDir = dir
	}
	if dataDir != "" {
		data, err := os.ReadFile(filepath.Join(dataDir, "server-port.txt"))
		if err == nil {
			// Rust's unsigned parser also accepts an optional leading plus.
			if port, err := strconv.ParseUint(strings.TrimPrefix(strings.TrimSpace(string(data)), "+"), 10, 16); err == nil {
				return uint16(port)
			}
		}
	}
	return defaultPort
}

type characterRequest struct {
	activate bool
	id       string
}

type characterResult struct {
	request characterRequest
	list    []characterEntry
	error   string
}

type errorTick struct{}

// model turns terminal and network events into pure app transitions and
// asynchronous commands.
type model struct {
	app
	ctx      context.Context
	api      apiClient
	events   <-chan any
	outgoing *outgoingQueue
	width    int
	height   int

	requests    []characterRequest
	restBusy    bool
	listPending bool
}

func (m *model) Init() tea.Cmd { return m.waitEvent }

func (m *model) waitEvent() tea.Msg {
	select {
	case event, ok := <-m.events:
		if ok {
			return event
		}
	case <-m.ctx.Done():
	}
	return tea.Quit()
}

func (m *model) Update(msg tea.Msg) (tea.Model, tea.Cmd) {
	m.tick(time.Now())
	switch msg := msg.(type) {
	case tea.WindowSizeMsg:
		m.width, m.height = msg.Width, msg.Height
	case connectionState:
		m.setConnection(msg)
		return m, m.waitEvent
	case serverMsg:
		m.applyServerMsg(msg)
		return m, m.waitEvent
	case tea.KeyMsg:
		wasConfig := m.inConfig()
		m.handleKey(msg)
		if m.shouldQuit {
			return m, tea.Quit
		}
		if m.inConfig() && (!wasConfig || len(m.characters) == 0) && !m.listPending {
			m.requests = append(m.requests, characterRequest{})
			m.listPending = true
		}
		return m, m.nextRequest()
	case characterResult:
		m.restBusy = false
		if !msg.request.activate {
			m.listPending = false
		}
		next := m.nextRequest()
		if msg.error != "" {
			m.setError(msg.error, time.Now())
			return m, tea.Batch(next, tea.Tick(errorTTL, func(time.Time) tea.Msg { return errorTick{} }))
		}
		m.setCharacters(msg.list)
		return m, next
	}
	return m, nil
}

func (m *model) View() string { return render(&m.app, m.width, m.height) }

func (m *model) nextRequest() tea.Cmd {
	if m.restBusy || len(m.requests) == 0 {
		return nil
	}
	req := m.requests[0]
	m.requests = m.requests[1:]
	m.restBusy = true
	// Keep REST off the event loop. Run activations and their refreshes in
	// key order so overlapping responses cannot restore stale active flags.
	api, ctx := m.api, m.ctx
	return func() tea.Msg {
		result := characterResult{request: req}
		prefix := "キャラクタ一覧の取得に失敗: "
		if req.activate {
			if _, err := api.activateCharacter(ctx, req.id); err != nil {
				result.error = "キャラクタの切替に失敗: " + err.Error()
				return result
			}
			prefix = "一覧の再取得に失敗: "
		}
		list, err := api.listCharacters(ctx)
		if err != nil {
			result.error = prefix + err.Error()
			return result
		}
		result.list = make([]characterEntry, 0, len(list))
		for _, entry := range list {
			result.list = append(result.list, characterEntry{entry.ID, entry.Name, entry.Active})
		}
		return result
	}
}

const (
	pageScrollLines = 20
	maxScrollOffset = 1<<16 - 1
)

func (m *model) handleKey(key tea.KeyMsg) {
	switch key.Type {
	case tea.KeyCtrlC:
		m.shouldQuit = true
		return
	case tea.KeyEsc:
		if m.inConfig() {
			m.returnToMain()
		} else {
			m.shouldQuit = true
		}
		return
	case tea.KeyF2:
		m.toggleView()
		return
	case tea.KeyTab:
		m.cycleFocus(1)
		return
	case tea.KeyShiftTab:
		m.cycleFocus(-1)
		return
	}
	if m.inConfig() {
		if m.focus == focusCharacters {
			switch key.Type {
			case tea.KeyUp, tea.KeyCtrlUp, tea.KeyShiftUp, tea.KeyCtrlShiftUp:
				m.moveCharacterCursor(-1)
			case tea.KeyDown, tea.KeyCtrlDown, tea.KeyShiftDown, tea.KeyCtrlShiftDown:
				m.moveCharacterCursor(1)
			case tea.KeyEnter:
				if entry := m.selectedCharacter(); entry != nil {
					m.setNotice(entry.name + " に切り替えています...")
					m.addEventLog("activate character: " + entry.name)
					m.requests = append(m.requests, characterRequest{activate: true, id: entry.id})
				}
			}
		}
		return
	}

	// Input focus scrolls the transcript so it can be read while typing.
	scroll := &m.chatScroll
	if m.focus == focusLog {
		scroll = &m.logScroll
	}
	switch key.Type {
	case tea.KeyUp, tea.KeyCtrlUp, tea.KeyShiftUp, tea.KeyCtrlShiftUp:
		*scroll = min(*scroll+1, maxScrollOffset)
		return
	case tea.KeyDown, tea.KeyCtrlDown, tea.KeyShiftDown, tea.KeyCtrlShiftDown:
		*scroll = max(*scroll-1, 0)
		return
	case tea.KeyPgUp, tea.KeyCtrlPgUp:
		*scroll = min(*scroll+pageScrollLines, maxScrollOffset)
		return
	case tea.KeyPgDown, tea.KeyCtrlPgDown:
		*scroll = max(*scroll-pageScrollLines, 0)
		return
	}
	if m.focus != focusInput {
		if key.Type == tea.KeyRunes && !key.Paste && strings.ContainsRune(string(key.Runes), 'q') {
			m.shouldQuit = true
		}
		return
	}

	switch key.Type {
	case tea.KeyEnter:
		if text := m.takeInputForSend(); text != "" {
			m.outgoing.send(clientMsg{Text: text})
		}
	case tea.KeyBackspace:
		_, size := utf8.DecodeLastRuneInString(m.input)
		m.input = m.input[:len(m.input)-size]
	case tea.KeySpace:
		m.input += " "
	case tea.KeyRunes:
		// Paste arrives as one batch in Bubble Tea. Control characters
		// are handled by key bindings and never become chat text.
		m.input += strings.Map(func(r rune) rune {
			if unicode.IsControl(r) {
				return -1
			}
			return r
		}, string(key.Runes))
	}
}
