//go:build darwin

package spool

import "golang.org/x/sys/unix"

// bootMs: kern.boottime is the boot timestamp directly — the same source
// libuv's os.uptime() derives from, so JS and Go boot tokens agree.
func bootMs() int64 {
	tv, err := unix.SysctlTimeval("kern.boottime")
	if err != nil {
		return 0
	}
	return int64(tv.Sec)*1000 + int64(tv.Usec)/1000
}
