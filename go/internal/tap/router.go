// Package tap is the Go always-on recorder: the session router and the
// recording proxy, ported from cli/tap.mjs + cli/proxy.mjs onto the
// protocol-complete internal/spool and golden-parity internal/normalize.
package tap

import (
	"sync"
	"time"
)

const (
	SessionHeader = "x-slink-session"
	LabelHeader   = "x-slink-label"
	AmbientKey    = "ambient"
	DefaultIdle   = 15 * time.Minute
)

// Router carves the continuous call stream into sessions — by cooperating
// clients' session header, else the idle-segmented ambient bucket. A busy
// session (call mid-stream) is alive however stale its last-seen time:
// idle is measured from request starts, and a stream can outlast the gap.
type Router struct {
	mu         sync.Mutex
	idle       time.Duration
	now        func() time.Time
	newSession func(name string) *Session
	onFinalize func(*Session)
	open       map[string]*routed
}

type routed struct {
	session *Session
	last    time.Time
}

func NewRouter(idle time.Duration, now func() time.Time, newSession func(string) *Session, onFinalize func(*Session)) *Router {
	if idle <= 0 {
		idle = DefaultIdle
	}
	if now == nil {
		now = time.Now
	}
	return &Router{
		idle: idle, now: now,
		newSession: newSession, onFinalize: onFinalize,
		open: map[string]*routed{},
	}
}

// Route returns the session a call on key belongs to, rolling stale idle
// sessions over — never busy ones.
func (r *Router) Route(key, name string) *Session {
	r.mu.Lock()
	defer r.mu.Unlock()
	t := r.now()
	if cur, ok := r.open[key]; ok {
		if t.Sub(cur.last) <= r.idle || cur.session.Busy() {
			cur.last = t
			return cur.session
		}
		delete(r.open, key)
		go r.onFinalize(cur.session)
	}
	s := r.newSession(name)
	r.open[key] = &routed{session: s, last: t}
	return s
}

// Sweep finalizes idle, non-busy sessions. Call on a timer.
func (r *Router) Sweep() {
	r.mu.Lock()
	var out []*Session
	t := r.now()
	for key, cur := range r.open {
		if t.Sub(cur.last) > r.idle && !cur.session.Busy() {
			delete(r.open, key)
			out = append(out, cur.session)
		}
	}
	r.mu.Unlock()
	for _, s := range out {
		go r.onFinalize(s)
	}
}

// FinalizeAll closes every open session (shutdown), synchronously.
func (r *Router) FinalizeAll() {
	r.mu.Lock()
	var out []*Session
	for key, cur := range r.open {
		delete(r.open, key)
		out = append(out, cur.session)
	}
	r.mu.Unlock()
	for _, s := range out {
		r.onFinalize(s)
	}
}

// Open snapshots the currently open sessions (for heartbeats).
func (r *Router) Open() []*Session {
	r.mu.Lock()
	defer r.mu.Unlock()
	out := make([]*Session, 0, len(r.open))
	for _, cur := range r.open {
		out = append(out, cur.session)
	}
	return out
}

// Size is the number of open sessions.
func (r *Router) Size() int {
	r.mu.Lock()
	defer r.mu.Unlock()
	return len(r.open)
}
