package mistclient

import (
	"fmt"
	"os"
	"os/exec"
	"syscall"
)

func configureCommand(cmd *exec.Cmd) {
	const createNoWindow = 0x08000000
	cmd.SysProcAttr = &syscall.SysProcAttr{CreationFlags: createNoWindow}
}

func exitStatus(status *os.ProcessState) string {
	code := uint32(status.ExitCode())
	if code&0x80000000 != 0 {
		return fmt.Sprintf("exit code: %#x", code)
	}
	return fmt.Sprintf("exit code: %d", code)
}
