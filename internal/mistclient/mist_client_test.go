package mistclient

import (
	"context"
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"runtime"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/config"
)

func TestFakeCLIContractErrorsAndTimeout(t *testing.T) {
	dir, mist := fixture(t)
	ctx := context.Background()
	cfg := readConfig(t, `{"providers":[
		{"id":"http","label":"HTTP <&>","base_url":"http://localhost/v1","api_key":"test-secret"},
		{"id":"r","base_url":"mist-network://team","provide":true,
		 "shared":[{"provider_id":"http","model":"model"}]}
	]}`)
	client := New(&mist)
	registration := BuildRegistration(&cfg)
	result, err := client.Apply(ctx, &registration)
	if err != nil || !slices.Equal(result.Warnings, []string{"test warning"}) {
		t.Fatalf("apply warnings = %q, error = %v", result.Warnings, err)
	}

	var args []string
	readFixtureJSON(t, filepath.Join(dir, "args.json"), &args)
	wantArgs := []string{"--instance", "normal instance", "--state-dir", dir, "ai", "external", "apply", "--owner", "tc-npc"}
	if !slices.Equal(args, wantArgs) {
		t.Fatalf("CLI args = %q, want %q", args, wantArgs)
	}
	stored, err := os.ReadFile(filepath.Join(dir, "registration.json"))
	if err != nil {
		t.Fatal(err)
	}
	const wantPayload = `{"owner":"tc-npc","label":"tc-npc","providers":[{"id":"http","label":"HTTP <&>","base_url":"http://localhost/v1","api_key":"test-secret","enabled":true}],"rooms":[{"room":"team","consume":true,"provide":true,"shared":[{"provider_id":"http","model":"model"}]}]}`
	if string(stored) != wantPayload {
		t.Fatal("CLI received different registration JSON")
	}
	live, err := client.Get(ctx)
	if err != nil {
		t.Fatal(err)
	}
	if len(live.Registrations) != 1 {
		t.Fatalf("wanted one applied registration, got %d", len(live.Registrations))
	}
	applied := live.Registrations[0]
	if len(applied.Status.Rooms) != 1 || applied.Status.Rooms[0].Peers != 2 {
		t.Fatalf("unexpected live room status: %+v", applied.Status)
	}
	if len(applied.Rooms) != 1 || len(applied.Rooms[0].Shared) != 1 || applied.Rooms[0].Shared[0].Model != "model" {
		t.Fatalf("unexpected applied room: %+v", applied.Rooms)
	}
	data, err := json.Marshal(live)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(string(data), "test-secret") || strings.Contains(string(data), `"providers"`) {
		t.Fatal("external get exposes provider information")
	}
	for _, want := range []bool{true, false} {
		if removed, err := client.Remove(ctx); err != nil || removed != want {
			t.Fatalf("remove = %t, %v, want %t", removed, err, want)
		}
	}
	if live, err = client.Get(ctx); err != nil || len(live.Registrations) != 0 {
		t.Fatalf("get after removal = %+v, %v", live, err)
	}

	for _, tc := range []struct{ mode, expected string }{
		{"fail", "daemon unavailable"},
		{"invalid", "invalid external API JSON"},
		{"slow", "mistl external API timed out"},
	} {
		t.Run(tc.mode, func(t *testing.T) {
			mist.Instance = &tc.mode
			_, err := New(&mist).WithTimeout(500 * time.Millisecond).Get(ctx)
			requireError(t, err, tc.expected)
			if tc.mode == "slow" && !errors.Is(err, context.DeadlineExceeded) {
				t.Fatalf("slow CLI error = %v, want context deadline", err)
			}
		})
	}

	mist.CLIPath = filepath.Join(dir, "missing.exe")
	_, err = New(&mist).Get(ctx)
	requireError(t, err, "could not start mistl; check mist.cli_path")
}

func TestCLIRegistrationOwnersAndRedaction(t *testing.T) {
	dir, mist := fixture(t)
	ctx := context.Background()
	mist.Instance = new("response")
	client := New(&mist)
	registration := Registration{Owner: Owner, Providers: []ExternalProvider{{APIKey: "test-secret"}}}
	respond := func(response string) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(dir, "response.json"), []byte(response), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	for _, response := range []string{`{"owner":"other","applied":true}`, `{"owner":"tc-npc","applied":false}`} {
		respond(response)
		_, err := client.Apply(ctx, &registration)
		requireError(t, err, "mistl did not acknowledge tc-npc registration")
	}
	respond(`{"owner":"tc-npc","applied":true,"warnings":["key test-secret was reported"]}`)
	result, err := client.Apply(ctx, &registration)
	if err != nil || !slices.Equal(result.Warnings, []string{"key *** was reported"}) {
		t.Fatal("apply did not redact the echoed key from warnings")
	}
	respond(`{"registrations":[{"owner":"other"},{"owner":"tc-npc"}]}`)
	live, err := client.Get(ctx)
	if err != nil || len(live.Registrations) != 1 || live.Registrations[0].Owner != Owner {
		t.Fatalf("get did not filter by owner: %+v, %v", live, err)
	}
	respond(`{"owner":"other","removed":true}`)
	_, err = client.Remove(ctx)
	requireError(t, err, "mistl returned a different registration owner")

	respond(`{"owner":"tc-npc","applied":"test-secret"}`)
	_, err = client.Apply(ctx, &registration)
	requireError(t, err, "mistl returned invalid external API JSON")
	if strings.Contains(err.Error(), "test-secret") {
		t.Fatal("parse error exposes the echoed key")
	}

	mist.Instance = new("fail-secret")
	_, err = New(&mist).Apply(ctx, &registration)
	status := "exit status: 2"
	if runtime.GOOS == "windows" {
		status = "exit code: 2"
	}
	want := "mistl ai external apply failed (" + status + "): key *** was rejected"
	if err == nil || err.Error() != want {
		t.Fatal("CLI failure did not report the exit status and redacted stderr")
	}
}

func TestCLIDiscovery(t *testing.T) {
	dir, mist := fixture(t)
	ctx := context.Background()
	pathDir := filepath.Join(dir, "path")
	localDir := filepath.Join(dir, "local app data")
	fallback := filepath.Join(localDir, "Programs", "mistl", "mistl.exe")
	if err := os.MkdirAll(pathDir, 0o700); err != nil {
		t.Fatal(err)
	}
	t.Setenv("PATH", pathDir)
	t.Setenv("LOCALAPPDATA", localDir)
	mist.CLIPath = "mistl"
	client := New(&mist)
	_, err := client.Get(ctx)
	requireError(t, err, "mistl not found; set mist.cli_path")

	installTestBinary(t, fallback)
	if _, err := client.Get(ctx); err != nil {
		t.Fatalf("LOCALAPPDATA discovery: %v", err)
	}
	mist.CLIPath = filepath.Join(dir, "missing.exe")
	_, err = New(&mist).Get(ctx)
	requireError(t, err, "could not start mistl; check mist.cli_path")

	if err := os.Remove(fallback); err != nil {
		t.Fatal(err)
	}
	installTestBinary(t, filepath.Join(pathDir, "mistl"+exeSuffix()))
	if _, err := client.Get(ctx); err != nil {
		t.Fatalf("PATH discovery: %v", err)
	}
}

func TestCLICancellation(t *testing.T) {
	dir, mist := fixture(t)
	mist.Instance = new("slow")
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	result := make(chan error, 1)
	go func() {
		_, err := New(&mist).Get(ctx)
		result <- err
	}()
	deadline := time.After(5 * time.Second)
	ticker := time.NewTicker(10 * time.Millisecond)
	defer ticker.Stop()
	for {
		if _, err := os.Stat(filepath.Join(dir, "args.json")); err == nil {
			break
		}
		select {
		case err := <-result:
			t.Fatalf("CLI stopped before cancellation: %v", err)
		case <-deadline:
			t.Fatal("CLI did not start")
		case <-ticker.C:
		}
	}
	cancel()
	select {
	case err := <-result:
		if !errors.Is(err, context.Canceled) {
			t.Fatalf("canceled CLI error = %v, want context cancellation", err)
		}
	case <-time.After(3 * time.Second):
		t.Fatal("CLI did not stop after cancellation")
	}
}

func TestCLIDrainsOutputWhileWritingStdin(t *testing.T) {
	_, mist := fixture(t)
	mist.Instance = new("verbose")
	registration := Registration{Owner: Owner, Label: strings.Repeat("x", 256<<10)}
	if _, err := New(&mist).Apply(context.Background(), &registration); err != nil {
		t.Fatal(err)
	}
}

func fixture(t *testing.T) (string, config.MistConfig) {
	t.Helper()
	t.Setenv("TC_NPC_FAKE_MISTL", "1")
	parent := t.TempDir()
	dir := filepath.Join(parent, "npc fake mistl")
	executable := filepath.Join(dir, "fake mistl"+exeSuffix())
	installTestBinary(t, executable)
	mist := config.DefaultMistConfig()
	mist.CLIPath = executable
	mist.Instance = new("normal instance")
	mist.StateDir = &dir
	return dir, mist
}

func installTestBinary(t *testing.T, destination string) {
	t.Helper()
	executable, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	data, err := os.ReadFile(executable)
	if err != nil {
		t.Fatal(err)
	}
	if err := os.MkdirAll(filepath.Dir(destination), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(destination, data, 0o700); err != nil {
		t.Fatal(err)
	}
}

func exeSuffix() string {
	if runtime.GOOS == "windows" {
		return ".exe"
	}
	return ""
}

func readFixtureJSON(t *testing.T, path string, target any) {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(data, target); err != nil {
		t.Fatal(err)
	}
}

func requireError(t *testing.T, err error, want string) {
	t.Helper()
	if err == nil || !strings.Contains(err.Error(), want) {
		t.Fatalf("error = %v, want %q", err, want)
	}
}
