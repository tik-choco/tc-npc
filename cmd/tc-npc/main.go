// Command tc-npc is a single binary unifying the former agent-talk /
// agent-memory / agent-speech / agent-vision / agent-action /
// agent-scheduler services plus an HTTP/WebSocket server for the web UI, all
// communicating over an in-process bus. See docs/ARCHITECTURE.md.
//
// One executable, four modes chosen by subcommand: app (default), mascot,
// serve (alias run), tui. app/mascot need a `-tags desktop,production`
// build; without it they return an error telling you to rebuild.
package main

import (
	"context"
	"errors"
	"fmt"
	"log/slog"
	"os"
	"os/signal"
	"strings"
	"sync"
	"syscall"

	"github.com/tik-choco/tc-npc/internal/action"
	"github.com/tik-choco/tc-npc/internal/bus"
	"github.com/tik-choco/tc-npc/internal/character"
	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/desktop"
	"github.com/tik-choco/tc-npc/internal/memory"
	"github.com/tik-choco/tc-npc/internal/mist"
	"github.com/tik-choco/tc-npc/internal/module"
	"github.com/tik-choco/tc-npc/internal/scheduler"
	"github.com/tik-choco/tc-npc/internal/server"
	"github.com/tik-choco/tc-npc/internal/speech"
	"github.com/tik-choco/tc-npc/internal/talk"
	"github.com/tik-choco/tc-npc/internal/translate"
	"github.com/tik-choco/tc-npc/internal/tui"
	"github.com/tik-choco/tc-npc/internal/vision"
)

const version = "0.1.0"

const usage = `tc-npc — unified AI mascot agent suite

Usage: tc-npc [--config <path>] [--no-open] [command]

Commands:
  app             Desktop app: agent suite/server + main window + mascot (default)
  mascot          Mascot window only, attaching to a running instance
  serve (run)     Agent suite and web server with no GUI
  tui [--addr A]  Terminal UI attached to a running instance
  import <path>   Import characters from a tc-town character export JSON
  characters      List stored characters
  config-path     Print the resolved config path and data directory

Global options:
  --config <path>  Path to config.json (default ~/.tc-npc/config.json)
  --no-open        Don't open the web UI in a browser at startup
  -V, --version    Print version
  -h, --help       Print help
`

type cli struct {
	configPath string
	noOpen     bool
	command    string
	args       []string
	addr       string
}

// parseArgs accepts the global flags anywhere on the command line, like
// clap's `global = true`.
func parseArgs(argv []string) (cli, error) {
	var c cli
	for i := 0; i < len(argv); i++ {
		a := argv[i]
		switch {
		case a == "-h" || a == "--help" || a == "help":
			fmt.Print(usage)
			os.Exit(0)
		case a == "-V" || a == "--version":
			fmt.Println("tc-npc", version)
			os.Exit(0)
		case a == "--no-open":
			c.noOpen = true
		case a == "--config":
			if i+1 >= len(argv) {
				return c, errors.New("--config requires a value")
			}
			i++
			c.configPath = argv[i]
		case strings.HasPrefix(a, "--config="):
			c.configPath = strings.TrimPrefix(a, "--config=")
		case a == "--addr":
			if i+1 >= len(argv) {
				return c, errors.New("--addr requires a value")
			}
			i++
			c.addr = argv[i]
		case strings.HasPrefix(a, "--addr="):
			c.addr = strings.TrimPrefix(a, "--addr=")
		case strings.HasPrefix(a, "-"):
			return c, fmt.Errorf("unknown option %q (see --help)", a)
		case c.command == "":
			c.command = a
		default:
			c.args = append(c.args, a)
		}
	}
	if c.command == "" {
		c.command = "app"
	}
	if c.addr != "" && c.command != "tui" {
		return c, errors.New("--addr is only valid for `tui`")
	}
	return c, nil
}

func main() {
	c, err := parseArgs(os.Args[1:])
	if err == nil {
		err = dispatch(c)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "Error:", err)
		os.Exit(1)
	}
}

func dispatch(c cli) error {
	switch c.command {
	case "app":
		return runApp(c.configPath, c.noOpen)
	case "mascot":
		return desktop.RunMascot("")
	case "serve", "run":
		return runServe(c.configPath, c.noOpen)
	case "tui":
		// No initLogging here: the TUI owns the terminal, and a log line
		// written into it from a background goroutine lands in the middle of
		// the drawn frame. Server activity shows in the TUI's own log panel.
		ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
		defer stop()
		return tui.Run(ctx, tui.Options{Addr: c.addr})
	case "import":
		if len(c.args) != 1 {
			return errors.New("usage: tc-npc import <path>")
		}
		return importCmd(c.configPath, c.args[0])
	case "characters":
		return listCharactersCmd(c.configPath)
	case "config-path":
		return configPathCmd(c.configPath)
	default:
		return fmt.Errorf("unknown command %q (see --help)", c.command)
	}
}

func initLogging() {
	level := slog.LevelInfo
	for _, env := range []string{"TC_NPC_LOG", "RUST_LOG"} {
		v := strings.ToLower(strings.TrimSpace(os.Getenv(env)))
		if v == "" {
			continue
		}
		switch {
		case strings.HasPrefix(v, "trace"), strings.HasPrefix(v, "debug"):
			level = slog.LevelDebug
		case strings.HasPrefix(v, "warn"):
			level = slog.LevelWarn
		case strings.HasPrefix(v, "error"):
			level = slog.LevelError
		}
		break
	}
	slog.SetDefault(slog.New(slog.NewTextHandler(os.Stderr, &slog.HandlerOptions{Level: level})))
}

// started is what startModules hands back: serve waits on a signal, app on
// the UI event loop; both cancel and then Wait.
type started struct {
	config *config.Config
	cancel context.CancelFunc
	wg     *sync.WaitGroup
}

func (s *started) stop() {
	slog.Info("shutdown requested, stopping modules...")
	s.cancel()
	s.wg.Wait()
}

// startModules loads config, wires the bus, and starts every enabled
// module. Shared by serve and app so the two cannot drift apart in what
// they bring up.
func startModules(configPath string, noOpen bool) (*started, error) {
	cfg, err := config.Load(configPath)
	if err != nil {
		return nil, err
	}
	if noOpen {
		cfg.Server.AutoOpen = false
	}
	resolved := config.ResolvePath(configPath)
	initLogging()

	dataDir := config.DataDir()
	if err := os.MkdirAll(dataDir, 0o755); err != nil {
		return nil, err
	}

	mc := module.Ctx{Bus: bus.New(), Config: cfg, ConfigPath: resolved, DataDir: dataDir}
	active, _ := character.ActiveCharacter(dataDir, cfg.Character.ActiveID)
	printBanner(cfg, active)

	ctx, cancel := context.WithCancel(context.Background())
	s := &started{config: cfg, cancel: cancel, wg: &sync.WaitGroup{}}
	spawn := func(name string, ctor func(module.Ctx) (module.Module, error)) {
		m, err := ctor(mc)
		if err != nil {
			// A module that can't start is not a reason to take the rest of
			// the process down with it.
			slog.Warn("module unavailable", "module", name, "error", err)
			return
		}
		s.wg.Add(1)
		go func() {
			defer s.wg.Done()
			if err := m.Run(ctx, mc); err != nil && !errors.Is(err, context.Canceled) {
				slog.Error("module exited with error", "module", name, "error", err)
			}
		}()
	}

	if cfg.Talk.Enabled {
		spawn("npc-talk", talk.New)
	}
	if cfg.Memory.Enabled {
		spawn("npc-memory", memory.New)
	}
	// Always spawned: it idles with both audio streams stopped when
	// tts/stt are off and starts them the moment the web UI turns either
	// on. Gating the spawn on startup config made the 音声 toggles appear
	// broken until a restart.
	spawn("npc-speech", speech.New)
	if cfg.Vision.Enabled {
		spawn("npc-vision", vision.New)
	}
	if cfg.Action.Enabled {
		spawn("npc-action", action.New)
	}
	// Always spawned: idles when disabled, picks up schedule edits live.
	spawn("npc-scheduler", scheduler.New)
	// Always spawned: idles when translation.mode is off, follows changes live.
	spawn("npc-translate", translate.New)
	if cfg.Mist.Enabled {
		spawn("npc-mist", mist.New)
	}
	// Always started: it is how the web UI, desktop windows and TUI connect.
	spawn("npc-server", server.New)

	return s, nil
}

func runServe(configPath string, noOpen bool) error {
	s, err := startModules(configPath, noOpen)
	if err != nil {
		return err
	}
	slog.Info("tc-npc running — press Ctrl+C to stop")
	sig := make(chan os.Signal, 1)
	signal.Notify(sig, os.Interrupt, syscall.SIGTERM)
	<-sig
	s.stop()
	return nil
}

func runApp(configPath string, noOpen bool) error {
	if !desktop.Available {
		return desktop.ErrNoDesktop
	}
	s, err := startModules(configPath, noOpen)
	if err != nil {
		return err
	}
	var once sync.Once
	stop := func() { once.Do(s.stop) }
	defer stop()
	return desktop.RunApp("http://"+s.config.Server.Addr, stop)
}

func printBanner(cfg *config.Config, active *character.Character) {
	fmt.Println("tc-npc starting")
	fmt.Printf("  talk=%s memory=%s tts=%s stt=%s vision=%s action=%s scheduler=%s mist=%s\n",
		onOff(cfg.Talk.Enabled), onOff(cfg.Memory.Enabled), onOff(cfg.TTS.Enabled),
		onOff(cfg.STT.Enabled), onOff(cfg.Vision.Enabled), onOff(cfg.Action.Enabled),
		onOff(cfg.Scheduler.Enabled), onOff(cfg.Mist.Enabled))
	fmt.Printf("  translation: %s\n", cfg.Translation.Mode())
	fmt.Printf("  server: http://%s\n", cfg.Server.Addr)
	if active != nil {
		fmt.Printf("  character: %s (%s)\n", active.Sheet.Name, active.ID)
	} else {
		fmt.Println("  character: (none active — see `tc-npc characters` / `character.active_id`)")
	}
}

func onOff(b bool) string {
	if b {
		return "on"
	}
	return "off"
}

func importCmd(configPath, path string) error {
	// Loaded only to fail fast on a broken config.
	if _, err := config.Load(configPath); err != nil {
		return err
	}
	dataDir := config.DataDir()
	if err := os.MkdirAll(dataDir, 0o755); err != nil {
		return err
	}
	data, err := os.ReadFile(path)
	if err != nil {
		return fmt.Errorf("failed to read %s: %w", path, err)
	}
	chars, err := character.ImportTCTownExportInto(string(data), dataDir)
	if err != nil {
		return err
	}
	if len(chars) == 0 {
		fmt.Printf("No characters found in %s\n", path)
		return nil
	}
	for i := range chars {
		if err := character.SaveCharacter(dataDir, &chars[i]); err != nil {
			return err
		}
		fmt.Printf("imported: %s  (%s)\n", chars[i].Sheet.Name, chars[i].ID)
	}
	fmt.Println()
	fmt.Println("hint: set `character.active_id` in your config to one of the ids above to make it the active character.")
	return nil
}

func listCharactersCmd(configPath string) error {
	cfg, err := config.Load(configPath)
	if err != nil {
		return err
	}
	chars, err := character.ListCharacters(config.DataDir())
	if err != nil {
		return err
	}
	if len(chars) == 0 {
		fmt.Println("No characters stored. Use `tc-npc import <path>` to import a tc-town export.")
		return nil
	}
	for _, ch := range chars {
		marker := " "
		if ch.ID == cfg.Character.ActiveID {
			marker = "*"
		}
		fmt.Printf("%s %s  %s\n", marker, ch.ID, ch.Sheet.Name)
	}
	return nil
}

func configPathCmd(configPath string) error {
	fmt.Printf("config: %s\n", config.ResolvePath(configPath))
	fmt.Printf("data dir: %s\n", config.DataDir())
	return nil
}
