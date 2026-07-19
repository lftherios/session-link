package tap

import (
	"os"
	"strconv"
	"time"

	"github.com/lftherios/session-link/internal/cli"
)

// autoPrune is the tap's rolling-buffer sweep at startup: ambient capture
// must not grow ~/.slink without bound. SLINK_RETAIN_DAYS (default 30)
// windows the age cut; empty captures go regardless of age. In-progress
// captures are never touched (PlanPrune's invariant).
func autoPrune(dir string) int {
	days := 30
	if v := os.Getenv("SLINK_RETAIN_DAYS"); v != "" {
		if n, err := strconv.Atoi(v); err == nil && n >= 0 {
			days = n
		}
	}
	captures := cli.ListCaptures(dir)
	remove, _ := cli.PlanPrune(captures, time.Now(), time.Duration(days)*24*time.Hour, -1, true)
	for _, c := range remove {
		cli.RemoveCapture(c.File)
	}
	return len(remove)
}
