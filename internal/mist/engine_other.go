//go:build !windows

package mist

import (
	"fmt"
	"runtime"
)

func loadEngine(string) (engine, error) {
	return nil, fmt.Errorf("npc-mist: mistlib DLL is not supported on %s (requires Windows)", runtime.GOOS)
}
