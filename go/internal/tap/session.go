package tap

import (
	"fmt"
	"log"
	"os"
	"sync"
	"sync/atomic"
	"time"

	"github.com/lftherios/session-link/internal/spool"
)

const cliLabel = "slink-go@0.0.0-dev"

// Session mirrors the JS recorder session: only the skeleton lives in
// memory; spans stream to the spool and are forgotten. The mutex serializes
// appends per session (the JS write chain), and id assignment happens
// under it so concurrent calls can't collide ids.
type Session struct {
	File string

	mu          sync.Mutex
	seq         int
	closed      bool
	lastEndedAt string
	skeleton    []byte
	pending     atomic.Int32
}

// NewSession builds the session skeleton exactly like proxy.mjs newSession.
func NewSession(dir, name string, now time.Time) *Session {
	created := now.UTC().Format("2006-01-02T15:04:05.000Z")
	skeleton := map[string]any{
		"schema":     "session/v0",
		"name":       name,
		"created_at": created,
		"source":     map[string]any{"kind": "proxy", "label": cliLabel, "fidelity": "exact"},
		"metadata":   map[string]any{"cwd": cwd(), "in_progress": true},
		"spans": []any{map[string]any{
			"id": "root", "parent_id": nil, "type": "agent",
			"name": name, "started_at": created,
		}},
	}
	sk, _ := spool.EncodeLine(skeleton)
	return &Session{File: spool.NewCapturePath(dir, now), skeleton: sk}
}

func cwd() string {
	d, err := os.Getwd()
	if err != nil {
		return ""
	}
	return d
}

// Busy reports in-flight calls — the router must not finalize under them.
func (s *Session) Busy() bool { return s.pending.Load() > 0 }

// Begin/End bracket a call assigned to this session.
func (s *Session) Begin() { s.pending.Add(1) }
func (s *Session) End()   { s.pending.Add(-1) }

// Record assigns the next span id under the lock, builds the span via
// build, and appends it — one serialized critical section, so ids are
// unique and spool order matches id order.
func (s *Session) Record(build func(id string) (map[string]any, string)) {
	s.mu.Lock()
	defer s.mu.Unlock()
	if s.closed {
		log.Printf("late span after session close — not recorded")
		return
	}
	id := fmt.Sprintf("s%d", s.seq+1)
	span, endedAt := build(id)
	line, err := spool.EncodeLine(span)
	if err != nil {
		log.Printf("capture encode failed: %v", err)
		return
	}
	s.seq++
	if endedAt != "" {
		s.lastEndedAt = endedAt
	}
	if err := spool.Append(s.File, [][]byte{line}, s.skeleton, func() bool { return s.closed }); err != nil {
		log.Printf("capture write failed: %v — recording may be incomplete", err)
	}
}

// Heartbeat refreshes the owner sidecar for a live, non-empty session.
func (s *Session) Heartbeat() {
	s.mu.Lock()
	active := s.seq > 0 && !s.closed
	s.mu.Unlock()
	if active {
		_ = spool.TouchOwnerSidecar(s.File)
	}
}

// Finalize closes the session and assembles its capture, with the
// protocol's retry-then-release behavior for a long-lived daemon.
func (s *Session) Finalize() {
	s.mu.Lock()
	s.closed = true
	seq := s.seq
	endedAt := s.lastEndedAt
	s.mu.Unlock()
	if seq == 0 {
		return // never recorded anything — nothing to save
	}
	if endedAt == "" {
		endedAt = time.Now().UTC().Format("2006-01-02T15:04:05.000Z")
	}
	for attempt := 1; ; attempt++ {
		n, err := spool.Assemble(s.File, spool.AssembleOptions{Finalize: true, EndedAt: endedAt})
		if err == nil {
			log.Printf("session ended · %d call(s) → %s", n, s.File)
			return
		}
		if err != spool.ErrCommitRetry {
			_ = spool.ReleaseOwnership(s.File)
			log.Printf("failed to save capture: %v (ownership released)", err)
			return
		}
		if attempt >= 5 {
			_ = spool.ReleaseOwnership(s.File)
			log.Printf("%v (ownership released — any slink run can finish it)", err)
			return
		}
		time.Sleep(2 * time.Second)
	}
}
