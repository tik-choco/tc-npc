//go:build !windows

package config

import (
	"os"
	"os/user"
)

func homeDir() string {
	if home, err := os.UserHomeDir(); err == nil {
		return home
	}
	if current, err := user.Current(); err == nil {
		return current.HomeDir
	}
	return ""
}
