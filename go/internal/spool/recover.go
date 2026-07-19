package spool

import (
	"crypto/rand"
	"encoding/hex"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"
)

// NewCapturePath mirrors the JS store's naming: <stamp>-<hex>.json — paths
// embed a timestamp plus 3 random bytes, so two sessions never collide.
func NewCapturePath(dir string, now time.Time) string {
	b := make([]byte, 3)
	rand.Read(b)
	stamp := now.UTC().Format("20060102-150405")
	return filepath.Join(dir, fmt.Sprintf("%s-%s.json", stamp, hex.EncodeToString(b)))
}

// RecoverDead finalizes spools whose recorder is dead, per the protocol's
// recovery pass: dead-owner spools become finished captures stamped with
// their last append time; structurally refused ones are set aside as
// .corrupt; ErrCommitRetry means a live-ish commit is in flight — skip.
// Returns the number recovered.
func RecoverDead(dir string) int {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return 0
	}
	recovered := 0
	names := map[string]bool{}
	for _, e := range entries {
		names[e.Name()] = true
	}
	for _, e := range entries {
		name := e.Name()
		// The protocol's litter sweeps: staging tmps, orphan sidecars,
		// abandoned locks (removed via the same atomic rename-first break
		// as the commit lock), and graves from crashed breakers.
		full := filepath.Join(dir, name)
		switch {
		case strings.HasSuffix(name, ".tmp"):
			sweepOlderThan(full, time.Hour, false)
			continue
		case strings.HasSuffix(name, ".json.spool.pid") && !names[strings.TrimSuffix(name, ".pid")]:
			sweepOlderThan(full, time.Hour, false)
			continue
		case strings.HasSuffix(name, ".json.lock"):
			sweepOlderThan(full, 10*time.Minute, true)
			continue
		case strings.HasSuffix(name, ".stale"):
			sweepOlderThan(full, 10*time.Minute, false)
			continue
		}
		if !strings.HasSuffix(name, ".json.spool") {
			continue
		}
		spoolFile := filepath.Join(dir, name)
		capture := strings.TrimSuffix(spoolFile, ".spool")
		if OwnerAlive(capture) {
			continue
		}
		st, err := os.Stat(spoolFile)
		if err != nil {
			continue
		}
		endedAt := st.ModTime().UTC().Format("2006-01-02T15:04:05.000Z")
		_, err = Assemble(capture, AssembleOptions{Finalize: true, EndedAt: endedAt})
		switch {
		case err == nil:
			recovered++
		case errors.Is(err, ErrRefused):
			os.Rename(spoolFile, spoolFile+".corrupt")
			os.Remove(spoolFile + ".pid")
		default:
			// ErrCommitRetry, transient fs errors: leave for a later pass.
		}
	}
	return recovered
}

func sweepOlderThan(path string, age time.Duration, viaRename bool) {
	st, err := os.Stat(path)
	if err != nil || time.Since(st.ModTime()) <= age {
		return
	}
	if !viaRename {
		os.Remove(path)
		return
	}
	grave := fmt.Sprintf("%s.%d.sweep.stale", path, os.Getpid())
	if os.Rename(path, grave) == nil {
		os.Remove(grave)
	}
}
