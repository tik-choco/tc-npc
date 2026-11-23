// Package server hosts the embedded web UI and the REST and WebSocket APIs.
package server

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"net"
	"net/http"
	"net/netip"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strconv"
	"sync"
	"sync/atomic"

	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/module"
)

const (
	maxBindAttempts      = 20
	serverPortFileName   = "server-port.txt"
	defaultBodyLimit     = 2 << 20
	maxVRMUploadBytes    = 200 << 20
	maxSpriteUploadBytes = 16 << 20
	maxAudioUploadBytes  = 64 << 20
)

type appState struct {
	ctx  context.Context
	mc   module.Ctx
	addr string
	hub  hub
	echo echoGuard
	mist mistSync

	// Readers share the latest immutable snapshot. Writers serialize the
	// read/modify/save/publish sequence across all config endpoints.
	current   atomic.Pointer[config.Config]
	configMu  sync.Mutex
	libraryMu sync.Mutex
	peopleMu  sync.Mutex

	cacheMu       sync.Mutex
	shortTerm     string
	affectHistory []affectEntry

	voiceActive atomic.Bool
	inputMu     sync.Mutex
	gate        suspendGate
	sockets     sync.WaitGroup
}

func newAppState(ctx context.Context, mc module.Ctx, addr string) *appState {
	s := &appState{ctx: ctx, mc: mc, addr: addr}
	s.current.Store(mc.Config)
	s.voiceActive.Store(true)
	return s
}

type serverModule struct{}

// New constructs the always-running npc-server module.
func New(_ module.Ctx) (module.Module, error) { return serverModule{}, nil }

func (serverModule) Name() string { return "npc-server" }

func (serverModule) Run(ctx context.Context, mc module.Ctx) error {
	listener, err := bindWithRetry(ctx, mc.Config.Server.Addr)
	if err != nil {
		return err
	}
	defer listener.Close()

	addr := listener.Addr().(*net.TCPAddr)
	slog.Info("npc-server: listening", "addr", addr)
	portFile := filepath.Join(mc.DataDir, serverPortFileName)
	if err := os.WriteFile(portFile, []byte(strconv.Itoa(addr.Port)), 0o666); err != nil {
		slog.Warn("npc-server: failed to write server-port.txt", "path", portFile, "error", err)
	} else {
		defer os.Remove(portFile)
	}

	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	s := newAppState(ctx, mc, addr.String())
	forwarded := s.startBusForwarder()
	synced := s.mist.start(ctx, mc.Config)
	defer func() {
		cancel()
		<-forwarded
		<-synced
	}()

	if mc.Config.Server.AutoOpen {
		url := "http://" + addr.String()
		if err := openTarget(ctx, url); err != nil {
			slog.Warn("npc-server: failed to auto-open browser", "url", url, "error", err)
		}
	}

	srv := &http.Server{
		Handler:                      s.routes(),
		BaseContext:                  func(net.Listener) context.Context { return ctx },
		DisableGeneralOptionsHandler: true,
	}
	done := make(chan error, 1)
	go func() { done <- srv.Serve(listener) }()
	select {
	case err := <-done:
		cancel()
		_ = srv.Shutdown(context.Background())
		s.sockets.Wait()
		return err
	case <-ctx.Done():
		err := srv.Shutdown(context.Background())
		s.sockets.Wait()
		if serveErr := <-done; !errors.Is(serveErr, http.ErrServerClosed) {
			return serveErr
		}
		return err
	}
}

func bindWithRetry(ctx context.Context, addr string) (net.Listener, error) {
	base, err := netip.ParseAddrPort(addr)
	if err != nil {
		return nil, fmt.Errorf("npc-server: invalid server.addr %q: %w", addr, err)
	}
	var lc net.ListenConfig
	network := "tcp"
	if base.Addr().Is4() {
		network = "tcp4"
	}
	for attempt := 0; attempt < maxBindAttempts; attempt++ {
		port := int(base.Port()) + attempt
		if port > 65535 {
			break
		}
		candidate := netip.AddrPortFrom(base.Addr(), uint16(port)).String()
		listener, err := lc.Listen(ctx, network, candidate)
		if err == nil {
			if attempt > 0 {
				slog.Info("npc-server: configured port was in use, bound to next available port", "configured", addr, "actual", listener.Addr())
			}
			return listener, nil
		}
		if !addrInUse(err) {
			return nil, err
		}
	}
	return nil, fmt.Errorf("npc-server: failed to bind after %d attempts starting at %s", maxBindAttempts, addr)
}

func openTarget(ctx context.Context, target string) error {
	var command *exec.Cmd
	switch runtime.GOOS {
	case "windows":
		command = exec.CommandContext(ctx, "rundll32", "url.dll,FileProtocolHandler", target)
	case "darwin":
		command = exec.CommandContext(ctx, "open", target)
	default:
		command = exec.CommandContext(ctx, "xdg-open", target)
	}
	return command.Run()
}
