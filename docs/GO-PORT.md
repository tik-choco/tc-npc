# Go port conventions (Rust → Go rewrite, branch `go-rewrite`)

tc-npc is being rewritten from Rust (`crates/`, `src/`) to Go for faster
development iteration. The Rust code stays in the tree as the reference until
the Go version reaches parity; then it is deleted. **Never edit or delete
Rust files** during the port.

Behaviour, wire formats and on-disk formats must stay identical to the Rust
version: the web UI (`web/`), `config.json` files written by the Rust build,
`~/.tc-npc/` data files and the WS/REST protocol in `docs/ARCHITECTURE.md`
must keep working unchanged.

## Layout

```
go.mod                       module github.com/tik-choco/tc-npc (Go 1.26, toolchain auto-downloaded)
cmd/tc-npc/                  main: CLI, wiring, startup        (← src/main.rs)
internal/bus/                in-process pub/sub bus             (← npc-core/bus.rs)
internal/module/             Module interface + Ctx             (← npc-core/module.rs)
internal/config/             Config schema, load/save, env      (← npc-core/config.rs)
internal/character/          character sheets, tc-town import   (← npc-core/character.rs)
internal/person/             person memory store                (← npc-core/person.rs)
internal/chatlog/            (← npc-core/chatlog.rs)
internal/vrm/  sound/  sprite/  osc/  scheduleprofile/  mistclient/
internal/llm/                OpenAI-compatible client           (← npc-llm)
internal/talk/ memory/ speech/ vision/ action/ scheduler/ translate/ server/
internal/tui/                terminal UI client                 (← npc-tui)
internal/mist/               mistlib DLL FFI                    (← npc-mist)
internal/desktop/            Wails v2, build tag `desktop`      (← src/desktop)
```

One Rust crate → one Go package (sub-packages allowed when it helps).
Rust module files map to Go files of the same name where sensible.
`internal/audio` is the cgo-free replacement for cpal/hound/symphonia
(WASAPI via go-wca); `web/embed.go` embeds `web/dist` (replaces rust-embed).

## Status (2026-11-23)

Every package above is ported, with the Rust tests ported alongside;
`go vet ./...` and `go test ./...` pass, and so do both builds (headless and
`-tags desktop,production`). Incremental rebuild ≈2 s.

Verified end to end on Windows (scratch data dir via `TC_NPC_DATA_DIR`):
- HTTP: `/healthz`, SPA + fallback, `/api/state`, redacted `/api/config`,
  `/api/audio/devices` (real WASAPI devices); route table matches Rust's.
- WS: `hello` → `input` → `inputAccepted` → `chat` → `response` with the same
  `requestId` (both the `done` and the `error` paths), `affect`, `translation`
  (assist mode, two targets) — against the real LLM endpoint.
- Voice: two instances over VB-Cable — TTS `ttsLine` per sentence +
  `speaking` edges on one, capture → VAD → STT → `sense` on the other.
- `cmd/affect-trace` output is byte-identical to the Rust example's.
- mistlib.dll v0.7.3-beta.6 loads (`get_version`, v2 event callback).

Still to check by hand before deleting the Rust tree:
- Desktop (Wails): mascot transparency, drag/resize, tray, window-state
  restore, child-process cleanup.
- mist with a real room (catalog discovery, Avatar Link with tc-streaming).
- Barge-in and priority clips / BGM on real speakers; vision capture; VRChat
  OSC actions.
- Web UI through the Go server for a full session (config save, VRM upload,
  people, scheduler test).

Go-only additions: `TC_NPC_DATA_DIR` overrides `~/.tc-npc` (dev/smoke
tests); `TC_NPC_LOG` sets the log level (`RUST_LOG` is still honoured).
Unlike rust-embed in debug builds, `web/dist` is always embedded at compile
time, so rebuild (≈2 s) after `vite build`.

Known pre-existing behaviour kept for parity: npc-translate (and npc-talk)
exit at startup when no model reference is configured, even with
`translation.mode = off`, so enabling translation later from the web UI does
nothing until restart.

## Conventions

- **Module shape**: each subsystem package exposes
  `func New(mc module.Ctx) (module.Module, error)`; `Run(ctx, mc)` returns
  when `ctx` is cancelled. `tokio_util::CancellationToken` → `context.Context`.
- **Bus**: `internal/bus`. `bus.Publish(topic, type, payload any)` (payload is
  JSON-marshalled; `json.RawMessage` passes through). `sub := b.Subscribe();
  defer sub.Close(); for msg := range sub.C`/`select`. Use the `bus.Topic*` /
  `bus.Msg*` constants. Payload is `json.RawMessage`; use `msg.Decode(&v)`,
  `msg.Map()`, `msg.Str(key)`.
- **Logging**: `log/slog` (`slog.Info/Warn/Error` with key/value pairs).
  `tracing::warn!(x = %y, "msg")` → `slog.Warn("msg", "x", y)`.
- **Errors**: `anyhow::Context` → `fmt.Errorf("context: %w", err)`.
- **JSON**: struct tags must reproduce serde exactly — field renames,
  `#[serde(default)]` (Go zero value or explicit defaults applied after
  unmarshal), `skip_serializing_if` (`omitempty` only when the Rust side
  skips; beware `omitempty` on bools/numbers that Rust always writes),
  `Option<T>` → pointer or `omitempty` as appropriate, tagged enums →
  custom `MarshalJSON`/`UnmarshalJSON`.
- **Tests**: port the Rust `#[cfg(test)]` tests and `crates/*/tests/` to
  `_test.go` files in the same package. Table-driven where natural.
- **Dependencies**: `go.mod` is shared by parallel workers. **Do not run
  `go get` or `go mod tidy`.** Already available: stdlib,
  `github.com/google/uuid`, `github.com/joho/godotenv`,
  `github.com/coder/websocket`, `github.com/charmbracelet/{bubbletea,bubbles,lipgloss}`,
  `github.com/mattn/go-runewidth`, `golang.org/x/sys`, `golang.org/x/text`,
  `golang.org/x/image` (resize: `x/image/draw`), `github.com/hajimehoshi/go-mp3`,
  `github.com/kbinani/screenshot`, `github.com/moutend/go-wca` +
  `github.com/go-ole/go-ole` (WASAPI audio, Windows).
  If you need something else, implement it with stdlib or stop and report
  the exact module path you need.
- **No cgo.** Everything must build with `CGO_ENABLED=0` (fast builds, no
  C toolchain). Platform-specific code goes behind build tags / `_windows.go`
  files with a stub for other OSes that returns a clear "not supported on
  this platform" error. The mistlib DLL is loaded with
  `golang.org/x/sys/windows` (`NewLazyDLL`, `syscall.NewCallback`), not cgo.
- **Concurrency**: `Arc<Mutex<T>>` → struct with `sync.Mutex`; spawned
  tasks → goroutines tied to `ctx`; `tokio::select!` → `select`.
  `spawn_blocking` → plain goroutine.
- **Comments**: keep the important *why* comments from the Rust source
  (they document hard-won behaviour); drop Rust-specific mechanics.
- **Verify**: `go build ./...` and `go test ./internal/<yourpkg>/...` and
  `go vet ./internal/<yourpkg>/...` must pass for your package. Other
  packages being written in parallel may not compile yet — only your own
  package(s) and their already-finished dependencies matter.
