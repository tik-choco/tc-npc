// Package mistclient implements mistl's external registration transport.
package mistclient

import (
	"bytes"
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"maps"
	"os"
	"os/exec"
	"path/filepath"
	"slices"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/tik-choco/tc-npc/internal/config"
)

// Owner identifies the registration managed by tc-npc.
const Owner = "tc-npc"

// Registration is the complete set of providers and rooms owned by tc-npc.
// It contains credentials and must not be logged.
type Registration struct {
	Owner     string             `json:"owner" required:"true"`
	Label     string             `json:"label" required:"true"`
	Providers []ExternalProvider `json:"providers" required:"true"`
	Rooms     []ExternalRoom     `json:"rooms" required:"true"`
}

// ExternalProvider is an HTTP provider referenced by a shared room model.
type ExternalProvider struct {
	ID      string `json:"id" required:"true"`
	Label   string `json:"label" required:"true"`
	BaseURL string `json:"base_url" required:"true"`
	APIKey  string `json:"api_key" required:"true"`
	Enabled bool   `json:"enabled" required:"true"`
}

// ExternalRoom describes how tc-npc participates in a room.
type ExternalRoom struct {
	Room    string            `json:"room" required:"true"`
	Consume bool              `json:"consume" required:"true"`
	Provide bool              `json:"provide" required:"true"`
	Shared  []config.ModelRef `json:"shared" required:"true"`
}

// BuildRegistration builds the registration from enabled rooms and usable shares.
// Ref order is retained because the first shared ref wins when two HTTP providers
// offer the same raw model ID.
func BuildRegistration(cfg *config.Config) Registration {
	http := make(map[string]*config.ProviderConfig)
	for i := range cfg.Providers {
		p := &cfg.Providers[i]
		if p.Enabled && (strings.HasPrefix(p.BaseURL, "http://") || strings.HasPrefix(p.BaseURL, "https://")) {
			http[p.ID] = p
		}
	}

	rooms := make(map[string]*ExternalRoom)
	used := make(map[string]*config.ProviderConfig)
	for i := range cfg.Providers {
		p := &cfg.Providers[i]
		room, ok := p.Room()
		if !p.Enabled || !ok {
			continue
		}
		entry := rooms[room]
		if entry == nil {
			entry = &ExternalRoom{Room: room, Consume: true, Shared: []config.ModelRef{}}
			rooms[room] = entry
		}
		entry.Provide = entry.Provide || p.Provide
		for _, ref := range p.Shared {
			provider := http[ref.ProviderID]
			if provider == nil || strings.TrimSpace(ref.Model) == "" {
				continue
			}
			used[ref.ProviderID] = provider
			if !slices.Contains(entry.Shared, ref) {
				entry.Shared = append(entry.Shared, ref)
			}
		}
	}

	registration := Registration{
		Owner:     Owner,
		Label:     Owner,
		Providers: make([]ExternalProvider, 0, len(used)),
		Rooms:     make([]ExternalRoom, 0, len(rooms)),
	}
	for _, id := range slices.Sorted(maps.Keys(used)) {
		p := used[id]
		registration.Providers = append(registration.Providers, ExternalProvider{
			ID: p.ID, Label: p.Label, BaseURL: p.BaseURL, APIKey: p.APIKey, Enabled: true,
		})
	}
	for _, room := range slices.Sorted(maps.Keys(rooms)) {
		registration.Rooms = append(registration.Rooms, *rooms[room])
	}
	return registration
}

// RoomBaseURL appends a room as one path segment, including escaped slash and percent.
func RoomBaseURL(base, room string) string {
	const hex = "0123456789ABCDEF"
	var url strings.Builder
	url.WriteString(strings.TrimRight(base, "/"))
	url.WriteString("/rooms/")
	for i := 0; i < len(room); i++ {
		b := room[i]
		switch {
		case b >= 'a' && b <= 'z', b >= 'A' && b <= 'Z', b >= '0' && b <= '9',
			b == '-', b == '.', b == '_', b == '~':
			url.WriteByte(b)
		default:
			url.WriteByte('%')
			url.WriteByte(hex[b>>4])
			url.WriteByte(hex[b&0xf])
		}
	}
	return url.String()
}

// RoomStatus is the daemon's live status for one room.
type RoomStatus struct {
	Room      string   `json:"room" required:"true"`
	Joined    bool     `json:"joined" required:"true"`
	Providing bool     `json:"providing" required:"true"`
	Peers     uint     `json:"peers" required:"true"`
	Models    []string `json:"models"`
}

// AppliedRegistration describes an applied registration without provider credentials.
type AppliedRegistration struct {
	Owner     string         `json:"owner" required:"true"`
	Rooms     []ExternalRoom `json:"rooms"`
	UpdatedAt *string        `json:"updated_at"`
	Status    ExternalStatus `json:"status"`
}

// ExternalStatus contains the live room statuses of a registration.
type ExternalStatus struct {
	Rooms []RoomStatus `json:"rooms"`
}

// ExternalGet is the credentials-free result of an external get.
type ExternalGet struct {
	Registrations []AppliedRegistration `json:"registrations" required:"true"`
}

// ApplyResult reports acknowledgment and warnings from an external apply.
type ApplyResult struct {
	Owner    string   `json:"owner" required:"true"`
	Applied  bool     `json:"applied" required:"true"`
	Warnings []string `json:"warnings"`
}

type removeResult struct {
	Owner   string `json:"owner" required:"true"`
	Removed bool   `json:"removed" required:"true"`
}

// MistClient invokes the external registration commands on the configured CLI.
type MistClient struct {
	path       string
	globalArgs []string
	timeout    time.Duration
}

// New constructs a client with the configured command and a 10-second timeout.
func New(cfg *config.MistConfig) *MistClient {
	client := &MistClient{path: cfg.CLIPath, timeout: 10 * time.Second}
	if cfg.Instance != nil {
		client.globalArgs = append(client.globalArgs, "--instance", *cfg.Instance)
	}
	if cfg.StateDir != nil {
		client.globalArgs = append(client.globalArgs, "--state-dir", *cfg.StateDir)
	}
	return client
}

// WithTimeout returns a copy with a different timeout for each operation.
func (c MistClient) WithTimeout(timeout time.Duration) *MistClient {
	c.timeout = timeout
	return &c
}

// Apply replaces the owner's registration and redacts submitted keys from warnings.
func (c *MistClient) Apply(ctx context.Context, payload *Registration) (ApplyResult, error) {
	data, err := payload.MarshalJSON()
	if err != nil {
		return ApplyResult{}, err
	}
	var secrets []string
	for _, p := range payload.Providers {
		if p.APIKey != "" {
			secrets = append(secrets, p.APIKey)
		}
	}
	var result ApplyResult
	if err := c.call(ctx, "apply", data, secrets, &result); err != nil {
		return ApplyResult{}, err
	}
	if result.Owner != Owner || !result.Applied {
		return ApplyResult{}, errors.New("mistl did not acknowledge tc-npc registration")
	}
	for i, warning := range result.Warnings {
		result.Warnings[i] = redact(warning, secrets)
	}
	return result, nil
}

// Remove removes the owner's registration and reports whether it existed.
func (c *MistClient) Remove(ctx context.Context) (bool, error) {
	var result removeResult
	if err := c.call(ctx, "remove", nil, nil, &result); err != nil {
		return false, err
	}
	if result.Owner != Owner {
		return false, errors.New("mistl returned a different registration owner")
	}
	return result.Removed, nil
}

// Get reads the applied registrations and retains only those owned by tc-npc.
func (c *MistClient) Get(ctx context.Context) (ExternalGet, error) {
	var result ExternalGet
	if err := c.call(ctx, "get", nil, nil, &result); err != nil {
		return ExternalGet{}, err
	}
	result.Registrations = slices.DeleteFunc(result.Registrations, func(registration AppliedRegistration) bool {
		return registration.Owner != Owner
	})
	return result, nil
}

func (c *MistClient) call(ctx context.Context, operation string, payload []byte, secrets []string, result any) error {
	ctx, cancel := context.WithTimeout(ctx, c.timeout)
	defer cancel()

	var stdout, stderr bytes.Buffer
	cmd, stdin, err := c.start(ctx, operation, &stdout, &stderr)
	if err != nil {
		if ctxErr := ctx.Err(); ctxErr != nil {
			return callContextError(ctxErr)
		}
		return err
	}

	// exec drains stdout and stderr while the writer sends stdin, so large
	// payloads cannot deadlock even if the child writes before reading.
	written := make(chan error, 1)
	go func() {
		var err error
		if len(payload) != 0 {
			_, err = stdin.Write(payload)
		}
		_ = stdin.Close()
		if err != nil {
			_ = cmd.Process.Kill()
		}
		written <- err
	}()
	waitErr := cmd.Wait()
	writeErr := <-written
	if err := ctx.Err(); err != nil {
		return callContextError(err)
	}
	if writeErr != nil {
		return fmt.Errorf("writing mistl stdin: %w", writeErr)
	}
	var exitErr *exec.ExitError
	if errors.As(waitErr, &exitErr) {
		message := strings.TrimSpace(strings.ToValidUTF8(stderr.String(), "\uFFFD"))
		return fmt.Errorf("mistl ai external %s failed (%s): %s", operation, exitStatus(exitErr.ProcessState), redact(message, secrets))
	}
	if waitErr != nil {
		return fmt.Errorf("waiting for mistl: %w", waitErr)
	}

	// Do not include stdout in parse errors: a faulty CLI might echo keys.
	if !utf8.Valid(stdout.Bytes()) {
		return errors.New("mistl returned invalid external API JSON: invalid UTF-8")
	}
	if err := json.Unmarshal(stdout.Bytes(), result); err != nil {
		if message := redact(err.Error(), secrets); message != err.Error() {
			return fmt.Errorf("mistl returned invalid external API JSON: %s", message)
		}
		return fmt.Errorf("mistl returned invalid external API JSON: %w", err)
	}
	return nil
}

func (c *MistClient) start(ctx context.Context, operation string, stdout, stderr io.Writer) (*exec.Cmd, io.WriteCloser, error) {
	args := slices.Concat(c.globalArgs, []string{"ai", "external", operation, "--owner", Owner})
	start := func(path string) (*exec.Cmd, io.WriteCloser, error) {
		cmd := exec.CommandContext(ctx, path, args...)
		cmd.Stdout, cmd.Stderr = stdout, stderr
		// Bound cleanup if a descendant holds one of the child's pipes open.
		cmd.WaitDelay = 100 * time.Millisecond
		configureCommand(cmd)
		stdin, err := cmd.StdinPipe()
		if err != nil {
			return nil, nil, fmt.Errorf("mistl stdin unavailable: %w", err)
		}
		return cmd, stdin, cmd.Start()
	}

	cmd, stdin, err := start(c.path)
	if err == nil {
		return cmd, stdin, nil
	}
	if c.path != "mistl" || !(errors.Is(err, exec.ErrNotFound) || errors.Is(err, os.ErrNotExist)) {
		return nil, nil, fmt.Errorf("could not start mistl; check mist.cli_path: %w", err)
	}
	local, ok := os.LookupEnv("LOCALAPPDATA")
	if !ok {
		return nil, nil, errors.New("mistl not found on PATH; set mist.cli_path")
	}
	cmd, stdin, err = start(filepath.Join(local, "Programs", "mistl", "mistl.exe"))
	if err != nil {
		return nil, nil, fmt.Errorf("mistl not found; set mist.cli_path: %w", err)
	}
	return cmd, stdin, nil
}

func callContextError(err error) error {
	if errors.Is(err, context.DeadlineExceeded) {
		return fmt.Errorf("mistl external API timed out: %w", err)
	}
	return err
}

func redact(message string, secrets []string) string {
	for _, key := range secrets {
		message = strings.ReplaceAll(message, key, "***")
	}
	return message
}
