package desktop

import (
	"os"
	"path/filepath"
	"testing"
)

func TestDesktopURLs(t *testing.T) {
	for _, raw := range []string{"http://127.0.0.1:47950", "http://127.0.0.1:47950/"} {
		t.Run(raw, func(t *testing.T) {
			base, err := serverBaseURL(raw)
			if err != nil {
				t.Fatal(err)
			}
			if got, want := base.String(), "http://127.0.0.1:47950/"; got != want {
				t.Errorf("main URL = %q, want %q", got, want)
			}
			avatar := mascotURL(base)
			if got, want := avatar.String(), "http://127.0.0.1:47950/#/avatar"; got != want {
				t.Errorf("mascot URL = %q, want %q", got, want)
			}
		})
	}
}

func TestRejectInvalidDesktopURLs(t *testing.T) {
	for _, raw := range []string{
		"", "127.0.0.1:47950", "http://127.0.0.1", "http://127.0.0.1:65536",
		"https://127.0.0.1:47950", "http://example.com:47950", "http://127.0.0.1:47950@localhost:80",
		"http://127.0.0.1:47950/elsewhere", "http://127.0.0.1:47950/?x=1", "http://127.0.0.1:47950/#/avatar",
	} {
		if _, err := serverBaseURL(raw); err == nil {
			t.Errorf("accepted invalid base URL %q", raw)
		}
	}
}

func TestDiscoverPort(t *testing.T) {
	dir := t.TempDir()
	if got := discoverPort(dir); got != defaultPort {
		t.Errorf("missing port file gave %d, want %d", got, defaultPort)
	}
	file := filepath.Join(dir, "server-port.txt")
	for text, want := range map[string]uint16{
		" 51234\r\n": 51234, "+51234": 51234, "65535": 65535, "0": 0,
		"": defaultPort, "bad": defaultPort, "-1": defaultPort, "65536": defaultPort,
	} {
		if err := os.WriteFile(file, []byte(text), 0o600); err != nil {
			t.Fatal(err)
		}
		if got := discoverPort(dir); got != want {
			t.Errorf("port file %q gave %d, want %d", text, got, want)
		}
	}
}
