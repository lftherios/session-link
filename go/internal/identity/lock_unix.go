//go:build !windows

package identity

import (
	"os"
	"syscall"
)

func lockDeviceFile(f *os.File) error {
	return syscall.Flock(int(f.Fd()), syscall.LOCK_EX|syscall.LOCK_NB)
}
