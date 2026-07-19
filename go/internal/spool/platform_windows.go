//go:build windows

package spool

// Windows recorders aren't shipped yet (P1 targets darwin/linux daemons);
// until then liveness degrades to heartbeat-freshness only: pids are
// assumed alive and the boot branch never matches.
func pidAlive(pid int) bool { return true }
func bootMs() int64         { return 0 }
