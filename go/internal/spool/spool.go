// Package spool implements the capture spool protocol (v1) — see
// docs/spool-protocol.md at the repo root. That document is the contract:
// it survived six adversarial verification rounds against the JS reference
// implementation (cli/store.mjs), and every rule exists because its absence
// was a demonstrated bug. Do not deviate from it without amending the doc.
package spool

import "time"

// Sidecar and lock naming, per the protocol's file table.
func SpoolPath(captureJSON string) string { return captureJSON + ".spool" }
func PidPath(captureJSON string) string   { return SpoolPath(captureJSON) + ".pid" }
func LockPath(captureJSON string) string  { return captureJSON + ".lock" }

// Protocol constants — values are part of the contract.
const (
	// SidecarFresh: heartbeat horizon; a sidecar younger than this marks a
	// live owner even when the boot token mismatches (wall-clock steps).
	SidecarFresh = 3 * time.Minute
	// BootTolerance: |writer boot − checker boot| within this matches.
	BootTolerance = 15 * time.Second
	// LockBreakAge: a commit lock older than this belongs to a crashed
	// holder and may be broken — by rename-to-grave only.
	LockBreakAge = 30 * time.Second
	// HeartbeatEvery: recorders refresh open sessions' sidecars on this
	// cadence (and on every append).
	HeartbeatEvery = 60 * time.Second
)

// OwnerSidecar is the JSON body of <capture>.json.spool.pid.
type OwnerSidecar struct {
	Pid  int   `json:"pid"`
	Boot int64 `json:"boot"` // ms; now − uptime at write time
}
