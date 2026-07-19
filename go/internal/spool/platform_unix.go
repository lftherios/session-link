//go:build unix

package spool

import (
	"errors"
	"os"
	"strconv"
	"strings"
	"syscall"
	"time"

	"golang.org/x/sys/unix"
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

// bootMs returns the machine's boot timestamp in ms — the same value the
// JS reference computes as Date.now() − os.uptime()·1000 (libuv reads
// kern.boottime on darwin and /proc/uptime on linux, both including
// sleep), so the two implementations' boot tokens agree within tolerance.
func bootMs() int64 {
	if tv, err := unix.SysctlTimeval("kern.boottime"); err == nil {
		return int64(tv.Sec)*1000 + int64(tv.Usec)/1000
	}
	if b, err := os.ReadFile("/proc/uptime"); err == nil {
		fields := strings.Fields(string(b))
		if len(fields) > 0 {
			if up, err := strconv.ParseFloat(fields[0], 64); err == nil {
				return time.Now().UnixMilli() - int64(up*1000)
			}
		}
	}
	return 0
}
