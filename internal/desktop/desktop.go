// Package desktop runs the tc-npc desktop clients.
package desktop

import (
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
)

const defaultPort = 47950

func discoverPort(dataDir string) uint16 {
	data, err := os.ReadFile(filepath.Join(dataDir, "server-port.txt"))
	if err == nil {
		// The Rust unsigned parser accepts a leading plus.
		port, err := strconv.ParseUint(strings.TrimPrefix(strings.TrimSpace(string(data)), "+"), 10, 16)
		if err == nil {
			return uint16(port)
		}
	}
	return defaultPort
}

func serverBaseURL(raw string) (url.URL, error) {
	base, err := url.Parse(raw)
	if err != nil {
		return url.URL{}, fmt.Errorf("parse desktop server URL: %w", err)
	}
	// The Rust capability is scoped to the loopback server. Keep the desktop
	// bridge on that origin, including the actual port selected by the server.
	if base.Scheme != "http" || base.Hostname() != "127.0.0.1" || base.User != nil ||
		(base.Path != "" && base.Path != "/") || base.RawPath != "" ||
		base.ForceQuery || base.RawQuery != "" || strings.Contains(raw, "#") {
		return url.URL{}, fmt.Errorf("desktop server URL must be an http://127.0.0.1:<port> base URL")
	}
	port, err := strconv.ParseUint(base.Port(), 10, 16)
	if err != nil {
		return url.URL{}, fmt.Errorf("parse desktop server port: %w", err)
	}
	base.Host = "127.0.0.1:" + strconv.FormatUint(port, 10)
	base.Path = "/"
	return *base, nil
}

func mascotURL(base url.URL) url.URL {
	base.Fragment = "/avatar"
	return base
}
