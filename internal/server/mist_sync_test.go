package server

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/tik-choco/tc-npc/internal/config"
	"github.com/tik-choco/tc-npc/internal/mistclient"
)

// The test binary doubles as the fake CLI, so tests require only Go and
// exercise the actual subprocess transport on Windows and Unix.
func TestMain(m *testing.M) {
	if len(os.Args) > 1 && os.Args[1] == "--instance" {
		if err := runFakeMist(os.Args[1:]); err != nil {
			fmt.Fprintln(os.Stderr, err)
			os.Exit(2)
		}
		os.Exit(0)
	}
	os.Exit(m.Run())
}

func runFakeMist(args []string) error {
	if len(args) != 9 || args[0] != "--instance" || args[2] != "--state-dir" ||
		!reflect.DeepEqual(args[4:6], []string{"ai", "external"}) ||
		!reflect.DeepEqual(args[7:], []string{"--owner", "tc-npc"}) {
		return fmt.Errorf("unexpected mistl arguments: %v", args)
	}
	dir := args[3]
	if err := os.WriteFile(filepath.Join(dir, "args.json"), []byte(args[1]), 0o600); err != nil {
		return err
	}
	switch args[1] {
	case "fail":
		return errors.New("error: daemon unavailable")
	case "delayed":
		time.Sleep(300 * time.Millisecond)
	case "normal instance":
	default:
		return fmt.Errorf("unexpected instance %q", args[1])
	}
	file := filepath.Join(dir, "registration.json")
	switch args[6] {
	case "apply":
		payload, err := io.ReadAll(os.Stdin)
		if err != nil {
			return err
		}
		if err := os.WriteFile(file, payload, 0o600); err != nil {
			return err
		}
		fmt.Println(`{"owner":"tc-npc","applied":true,"warnings":["test warning"]}`)
	case "remove":
		fmt.Printf("{\"owner\":\"tc-npc\",\"removed\":%t}\n", os.Remove(file) == nil)
	case "get":
		data, err := os.ReadFile(file)
		if errors.Is(err, os.ErrNotExist) {
			fmt.Println(`{"registrations":[]}`)
			return nil
		}
		if err != nil {
			return err
		}
		var registration object
		if err := json.Unmarshal(data, &registration); err != nil {
			return err
		}
		registration["updated_at"] = "2026-10-04T00:00:00Z"
		registration["status"] = object{"rooms": []object{{
			"room": "team", "joined": true, "providing": true, "peers": 2, "models": []string{"model"},
		}}}
		return json.NewEncoder(os.Stdout).Encode(object{"registrations": []object{registration}})
	default:
		return fmt.Errorf("unexpected mistl operation %q", args[6])
	}
	return nil
}

func TestExternalRegistrationStartSaveRemoveAndFailure(t *testing.T) {
	s := testState(t)
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	// Exercise argument quoting, including spaces in the instance and path.
	dir := filepath.Join(s.mc.DataDir, "mist state")
	if err := os.Mkdir(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	cfg := decodeResponse[config.Config](t, []byte(`{"providers":[
		{"id":"h","base_url":"http://localhost/v1","api_key":"test-secret"},
		{"id":"r","base_url":"mist-network://team","provide":true,"shared":[{"provider_id":"h","model":"old"}]}
	]}`))
	cfg.Mist.CLIPath, cfg.Mist.Instance, cfg.Mist.StateDir = exe, pointer("delayed"), &dir
	s.current.Store(&cfg)
	ctx, cancel := context.WithCancel(s.ctx)
	done := s.mist.start(ctx, &cfg)
	t.Cleanup(func() { cancel(); <-done })
	waitUntil(t, func() bool {
		_, err := os.Stat(filepath.Join(dir, "args.json"))
		return err == nil
	})

	h := s.routes()
	// Save during startup apply. The UI's masked key is restored and the
	// later generation must win even if the startup call finishes first.
	updated := decodeResponse[config.Config](t, []byte(jsonText(t, &cfg)))
	updated.Mist.Instance = pointer("normal instance")
	updated.Providers[1].Shared[0].Model = "model"
	redacted, err := updated.RedactedJSON()
	if err != nil {
		t.Fatal(err)
	}
	request(t, h, http.MethodPut, "/api/config", jsonText(t, redacted), http.StatusOK)
	waitUntil(t, func() bool { return !s.mist.status().Pending })
	w := request(t, h, http.MethodGet, "/api/mist/sync", "", http.StatusOK)
	if status := decodeResponse[syncStatus](t, w.Body.Bytes()); !status.Applied || status.Generation != 2 ||
		!reflect.DeepEqual(status.Warnings, []string{"test warning"}) {
		t.Fatalf("sync status: %s", w.Body)
	}
	file := filepath.Join(dir, "registration.json")
	raw, err := os.ReadFile(file)
	if err != nil {
		t.Fatal(err)
	}
	payload := decodeResponse[mistclient.Registration](t, raw)
	if payload.Rooms[0].Shared[0].Model != "model" || payload.Providers[0].APIKey != "test-secret" {
		t.Fatalf("registration: %s", raw)
	}
	w = request(t, h, http.MethodGet, "/api/mist/rooms", "", http.StatusOK)
	live := decodeResponse[mistclient.ExternalGet](t, w.Body.Bytes())
	if live.Registrations[0].Status.Rooms[0].Peers != 2 ||
		strings.Contains(w.Body.String(), "test-secret") || strings.Contains(w.Body.String(), "\"providers\"") {
		t.Fatalf("live rooms: %s", w.Body)
	}

	updated.Providers[1].Enabled = false
	request(t, h, http.MethodPut, "/api/config", jsonText(t, &updated), http.StatusOK)
	waitUntil(t, func() bool { return !s.mist.status().Pending })
	if _, err := os.Stat(file); !os.IsNotExist(err) || s.mist.status().Applied {
		t.Fatalf("registration removal failed: %v", err)
	}

	updated.Mist.Instance = pointer("fail")
	request(t, h, http.MethodPut, "/api/config", jsonText(t, &updated), http.StatusOK)
	waitUntil(t, func() bool { return !s.mist.status().Pending })
	if status := s.mist.status(); !strings.Contains(valueOr(status.Error, ""), "daemon unavailable") {
		t.Fatalf("expected reported daemon failure, got %+v", status)
	}
	request(t, h, http.MethodGet, "/api/mist/rooms", "", http.StatusBadGateway)
	if _, err := os.Stat(s.mc.ConfigPath); err != nil {
		t.Fatal(err)
	}
}
