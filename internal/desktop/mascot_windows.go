//go:build desktop && production

package desktop

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"os/exec"
	"time"

	"github.com/tik-choco/tc-npc/internal/config"
	wailsruntime "github.com/wailsapp/wails/v2/pkg/runtime"
)

// The parent passes its selected URL through the child's environment so the
// regular mascot subcommand can call RunMascot("") without a private CLI flag.
const mascotServerEnv = "TC_NPC_DESKTOP_MASCOT_SERVER_URL"

var allowSetForegroundWindow = user32.NewProc("AllowSetForegroundWindow")

// RunMascot opens only the mascot window on the main goroutine. An empty
// serverURL discovers the local server unless the parent supplied its URL.
func RunMascot(serverURL string) error {
	parentURL := os.Getenv(mascotServerEnv)
	dataDir := config.DataDir()
	if serverURL == "" {
		serverURL = parentURL
	}
	if serverURL == "" {
		serverURL = fmt.Sprintf("http://127.0.0.1:%d", discoverPort(dataDir))
	}
	base, err := serverBaseURL(serverURL)
	if err != nil {
		return err
	}
	mode := windowMode{
		title: "tc-npc mascot", stateKey: "standalone-mascot", legacyLabel: "main",
		width: 360, height: 480, mascot: true, managed: parentURL != "",
	}
	if mode.managed {
		mode.stateKey, mode.legacyLabel = "mascot", "mascot"
	}
	return runWindow(mascotURL(base), dataDir, mode, nil)
}

type mascotChild struct {
	cmd   *exec.Cmd
	stdin *os.File
	done  chan struct{}
}

func startMascot(serverURL string) (*mascotChild, error) {
	executable, err := os.Executable()
	if err != nil {
		return nil, fmt.Errorf("locate tc-npc executable: %w", err)
	}
	read, write, err := os.Pipe()
	if err != nil {
		return nil, fmt.Errorf("create mascot control pipe: %w", err)
	}
	defer read.Close()
	cmd := exec.Command(executable, "mascot")
	cmd.Env = append(os.Environ(), mascotServerEnv+"="+serverURL)
	cmd.Stdin, cmd.Stdout, cmd.Stderr = read, os.Stdout, os.Stderr
	if err := cmd.Start(); err != nil {
		write.Close()
		return nil, fmt.Errorf("start mascot: %w", err)
	}
	child := &mascotChild{cmd: cmd, stdin: write, done: make(chan struct{})}
	go func() {
		err := cmd.Wait()
		close(child.done)
		if err != nil {
			slog.Debug("desktop: mascot process exited", "error", err)
		}
	}()
	return child, nil
}

func (child *mascotChild) send(command string) {
	if command == "show" {
		// Pass the tray's foreground activation to the separate GUI process.
		allowSetForegroundWindow.Call(uintptr(child.cmd.Process.Pid))
	}
	if _, err := fmt.Fprintln(child.stdin, command); err != nil {
		select {
		case <-child.done:
		default:
			if !errors.Is(err, os.ErrClosed) {
				slog.Warn("desktop: send mascot control", "error", err)
			}
		}
	}
}

func (child *mascotChild) stop() {
	// EOF lets the child save its geometry. Kill and reap it if the UI
	// is stuck. An abrupt parent exit also closes its end of the pipe.
	_ = child.stdin.Close()
	deadline := time.NewTimer(3 * time.Second)
	defer deadline.Stop()
	select {
	case <-child.done:
	case <-deadline.C:
		if err := child.cmd.Process.Kill(); err != nil && !errors.Is(err, os.ErrProcessDone) {
			slog.Warn("desktop: kill mascot", "error", err)
			return
		}
		<-child.done
	}
}

func readMascotCommands(lifetime, ctx context.Context, window windowInvoker) {
	scanner := bufio.NewScanner(os.Stdin)
	for scanner.Scan() {
		if lifetime.Err() != nil {
			return
		}
		switch scanner.Text() {
		case "show":
			window.Invoke(func() { wailsruntime.WindowShow(ctx) })
		case "hide":
			window.Invoke(func() { wailsruntime.WindowHide(ctx) })
		}
	}
	if lifetime.Err() == nil {
		window.Invoke(func() { wailsruntime.Quit(ctx) })
	}
}
