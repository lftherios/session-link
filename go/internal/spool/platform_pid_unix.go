//go:build unix

package spool

import (
	"errors"
	"syscall"
)

// pidAlive: signal-0 semantics per the protocol — EPERM means the process
// EXISTS under another uid, which is alive.
func pidAlive(pid int) bool {
	err := syscall.Kill(pid, 0)
	if err == nil {
		return true
	}
	return errors.Is(err, syscall.EPERM)
}

// The boot token is meaningful on unix — both implementations read the
// same kernel sources, so writer and checker agree within tolerance.
const bootTokenReliable = true
