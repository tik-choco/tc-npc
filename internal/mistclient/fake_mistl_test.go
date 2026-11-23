package mistclient

import (
	"bytes"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"
)

func TestMain(m *testing.M) {
	if os.Getenv("TC_NPC_FAKE_MISTL") == "1" {
		fakeMistl(os.Args[1:])
		os.Exit(0)
	}
	os.Exit(m.Run())
}

// fakeMistl runs in the test executable and never invokes a real mistl daemon.
func fakeMistl(args []string) {
	if len(args) != 9 || args[0] != "--instance" || args[2] != "--state-dir" ||
		!slices.Equal(args[4:6], []string{"ai", "external"}) ||
		!slices.Equal(args[7:], []string{"--owner", "tc-npc"}) {
		panic(fmt.Sprintf("unexpected arguments: %q", args))
	}
	dir := args[3]
	argJSON, err := json.Marshal(args)
	mustFakeMistl(err)
	mustFakeMistl(os.WriteFile(filepath.Join(dir, "args.json"), argJSON, 0o600))

	switch args[1] {
	case "fail":
		fmt.Fprintln(os.Stderr, "error: daemon unavailable")
		os.Exit(2)
	case "invalid":
		fmt.Println("not JSON")
		return
	case "slow":
		time.Sleep(30 * time.Second)
		return
	case "delayed":
		time.Sleep(300 * time.Millisecond)
	case "verbose":
		_, err := io.WriteString(os.Stderr, strings.Repeat("x", 256<<10))
		mustFakeMistl(err)
	case "normal instance", "response", "fail-secret":
	default:
		panic("unexpected instance: " + args[1])
	}

	payload, err := io.ReadAll(os.Stdin)
	mustFakeMistl(err)
	if args[6] != "apply" && len(payload) != 0 {
		panic("unexpected stdin payload")
	}
	switch args[1] {
	case "response":
		response, err := os.ReadFile(filepath.Join(dir, "response.json"))
		mustFakeMistl(err)
		fmt.Print(string(response))
		return
	case "fail-secret":
		fmt.Fprintln(os.Stderr, "  key test-secret was rejected  ")
		os.Exit(2)
	}

	file := filepath.Join(dir, "registration.json")
	switch args[6] {
	case "apply":
		if !bytes.Contains(payload, []byte(`"owner":"tc-npc"`)) {
			panic("registration is missing the owner")
		}
		mustFakeMistl(os.WriteFile(file, payload, 0o600))
		fmt.Println(`{"owner":"tc-npc","applied":true,"warnings":["test warning"]}`)
	case "remove":
		fmt.Printf("{\"owner\":\"tc-npc\",\"removed\":%t}\n", os.Remove(file) == nil)
	case "get":
		stored, err := os.ReadFile(file)
		if err != nil {
			fmt.Println(`{"registrations":[]}`)
			return
		}
		stored = bytes.ReplaceAll(stored, []byte("test-secret"), []byte("***"))
		stored = bytes.TrimSuffix(stored, []byte("}"))
		fmt.Printf(`{"registrations":[%s,"updated_at":"2026-10-04T00:00:00Z","status":{"rooms":[{"room":"team","joined":true,"providing":true,"peers":2,"models":["model"]}]}}]}`+"\n", stored)
	default:
		panic("unexpected operation: " + args[6])
	}
}

func mustFakeMistl(err error) {
	if err != nil {
		panic(err)
	}
}
