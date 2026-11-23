//go:build !windows

package mistclient

import (
	"fmt"
	"os"
	"os/exec"
)

func configureCommand(*exec.Cmd) {}

func exitStatus(status *os.ProcessState) string {
	if code := status.ExitCode(); code >= 0 {
		return fmt.Sprintf("exit status: %d", code)
	}
	return status.String()
}
