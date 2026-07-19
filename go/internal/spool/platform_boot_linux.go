//go:build linux

package spool

import (
	"os"
	"strconv"
	"strings"
	"time"
)

// bootMs: now − /proc/uptime (which includes suspend, per proc(5)) — the
// same computation the JS reference makes via os.uptime().
func bootMs() int64 {
	b, err := os.ReadFile("/proc/uptime")
	if err != nil {
		return 0
	}
	fields := strings.Fields(string(b))
	if len(fields) == 0 {
		return 0
	}
	up, err := strconv.ParseFloat(fields[0], 64)
	if err != nil {
		return 0
	}
	return time.Now().UnixMilli() - int64(up*1000)
}
