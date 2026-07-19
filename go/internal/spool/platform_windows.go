//go:build windows

package spool

// Windows recorders aren't shipped yet (P1 targets darwin/linux daemons).
// Liveness degrades to heartbeat-freshness only: pids are assumed alive
// and the boot branch is disabled outright — a 0==0 comparison would
// otherwise read every windows-written sidecar as boot-matching forever,
// pinning dead recorders' spools permanently.
func pidAlive(pid int) bool { return true }
func bootMs() int64         { return 0 }

const bootTokenReliable = false
