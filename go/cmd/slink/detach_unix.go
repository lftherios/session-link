//go:build unix

package main

import (
	"os/exec"
	"syscall"
)

// detach puts the daemon in its own session so it survives the parent
// shell exiting.
func detach(cmd *exec.Cmd) {
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
}
