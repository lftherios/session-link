// The spool engine — the Go implementation of docs/spool-protocol.md.
// Parity with the JS reference (cli/store.mjs) is enforced by the
// testdata/spool goldens: PARSED equality of the assembled document
// (assembled bytes are implementation-specific — Go marshals maps in
// key order — the JSON value is the contract).
package spool

import (
	"bufio"
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"
)

var (
	// ErrRefused: the spool's first parseable line is not a run skeleton
	// (no .schema). Structural — recovery sets the spool aside as .corrupt.
	ErrRefused = errors.New("spool refused: first line is not a run skeleton")
	// ErrCommitRetry: the commit aborted to protect data (spool grew, lock
	// lost). Transient — retry later; NEVER treat as corrupt.
	ErrCommitRetry = errors.New("capture commit aborted — will retry on a later pass")
	// ErrLockStuck: a fresh-but-stuck lock outlived the wait budget.
	ErrLockStuck = errors.New("capture commit lock is stuck")
)

// EncodeLine marshals one spool line, escaping U+2028/U+2029 — Go's
// encoding/json leaves them raw (they're valid JSON string chars), but
// line-based readers treat them as line breaks and would tear the span.
func EncodeLine(v any) ([]byte, error) {
	var buf bytes.Buffer
	enc := json.NewEncoder(&buf)
	enc.SetEscapeHTML(false)
	if err := enc.Encode(v); err != nil { // Encode appends the \n we want
		return nil, err
	}
	b := buf.Bytes()
	b = bytes.ReplaceAll(b, []byte("\u2028"), []byte(`\u2028`))
	b = bytes.ReplaceAll(b, []byte("\u2029"), []byte(`\u2029`))
	return b, nil
}

// Append adds span lines to the capture's spool, per the protocol: the
// sidecar is heartbeat-touched first; the skeleton header is laid whenever
// the spool is missing or empty; every append to a non-empty spool starts
// on a fresh line; isClosed is the last gate before bytes land.
func Append(captureJSON string, spans [][]byte, skeleton []byte, isClosed func() bool) error {
	if err := os.MkdirAll(filepath.Dir(captureJSON), 0o755); err != nil {
		return err
	}
	spool := SpoolPath(captureJSON)
	var size int64
	if st, err := os.Stat(spool); err == nil {
		size = st.Size()
	}
	if isClosed != nil && isClosed() {
		return nil // session finalized under us — drop, never respool
	}
	if err := TouchOwnerSidecar(captureJSON); err != nil {
		return err
	}
	var payload bytes.Buffer
	if size > 0 {
		payload.WriteByte('\n')
	} else {
		payload.Write(skeleton) // EncodeLine output: newline included
	}
	for _, s := range spans {
		payload.Write(s)
	}
	f, err := os.OpenFile(spool, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return err
	}
	defer f.Close()
	_, err = f.Write(payload.Bytes())
	return err
}

// TouchOwnerSidecar atomically (re)writes {pid, boot} — the heartbeat.
func TouchOwnerSidecar(captureJSON string) error {
	body, err := json.Marshal(OwnerSidecar{Pid: os.Getpid(), Boot: bootMs()})
	if err != nil {
		return err
	}
	return atomicWrite(PidPath(captureJSON), body)
}

// ReleaseOwnership deletes the sidecar so any process may finalize.
func ReleaseOwnership(captureJSON string) error {
	err := os.Remove(PidPath(captureJSON))
	if errors.Is(err, os.ErrNotExist) {
		return nil
	}
	return err
}

// OwnerAlive implements the liveness rule:
// alive := pidAlive && (sidecarFresh || (bootMatches && pid != 1))
// Legacy bare-integer sidecars keep plain pid semantics.
func OwnerAlive(captureJSON string) bool {
	p := PidPath(captureJSON)
	raw, err := os.ReadFile(p)
	if err != nil {
		return false
	}
	st, err := os.Stat(p)
	if err != nil {
		return false
	}
	trimmed := strings.TrimSpace(string(raw))
	var pid int
	var boot *int64
	if strings.HasPrefix(trimmed, "{") {
		var sc struct {
			Pid  int    `json:"pid"`
			Boot *int64 `json:"boot"`
		}
		if json.Unmarshal([]byte(trimmed), &sc) != nil {
			return false
		}
		pid, boot = sc.Pid, sc.Boot
	} else {
		n, err := strconv.Atoi(trimmed)
		if err != nil {
			return false
		}
		pid = n
	}
	if pid <= 0 || !pidAlive(pid) {
		return false
	}
	if boot == nil {
		return true // legacy sidecar: plain pid semantics
	}
	fresh := time.Since(st.ModTime()) <= SidecarFresh
	bootMatches := absDiff(*boot, bootMs()) <= BootTolerance.Milliseconds()
	return fresh || (bootMatches && pid != 1)
}

// AssembleOptions selects the commit mode, per the protocol.
type AssembleOptions struct {
	Finalize bool
	EndedAt  string // RFC3339 ms; finalize mode
}

// Assemble streams the spool into the capture's .json. Returns the span
// count. (0, nil) with no error never happens: no spool or nothing
// salvageable returns os.ErrNotExist / ErrRefused; protective aborts
// return ErrCommitRetry.
func Assemble(captureJSON string, opts AssembleOptions) (int, error) {
	spool := SpoolPath(captureJSON)
	st, err := os.Stat(spool)
	if err != nil {
		return 0, os.ErrNotExist
	}
	spoolSizeAtStart := st.Size()
	spoolMtime := st.ModTime()

	var jsonBefore *time.Time
	if jst, err := os.Stat(captureJSON); err == nil {
		t := jst.ModTime()
		jsonBefore = &t
	}

	in, err := os.Open(spool)
	if err != nil {
		return 0, os.ErrNotExist
	}
	defer in.Close()

	tmp := fmt.Sprintf("%s.%s.tmp", captureJSON, randHex(4))
	out, err := os.Create(tmp)
	if err != nil {
		return 0, err
	}
	discard := func() {
		out.Close()
		os.Remove(tmp)
	}

	w := bufio.NewWriter(out)
	r := bufio.NewReader(in)
	var skeleton map[string]json.RawMessage
	spans := 0
	for {
		line, rerr := r.ReadString('\n')
		line = strings.TrimSuffix(line, "\n")
		if s := strings.TrimSpace(line); s != "" {
			var obj map[string]json.RawMessage
			if json.Unmarshal([]byte(s), &obj) != nil {
				// partial/torn line — drop it
			} else if skeleton == nil {
				var schema string
				if obj["schema"] == nil || json.Unmarshal(obj["schema"], &schema) != nil || schema == "" {
					discard()
					return 0, ErrRefused
				}
				skeleton = obj
				if err := writeHeader(w, skeleton, opts); err != nil {
					discard()
					return 0, err
				}
			} else {
				spans++
				w.WriteString(",")
				w.WriteString(s)
			}
		}
		if rerr == io.EOF {
			break
		}
		if rerr != nil {
			discard()
			return 0, rerr
		}
	}
	if skeleton == nil || spans == 0 {
		discard()
		return 0, ErrRefused
	}
	w.WriteString("]}")
	if err := w.Flush(); err != nil {
		discard()
		return 0, err
	}
	if err := out.Close(); err != nil {
		os.Remove(tmp)
		return 0, err
	}

	// The commit — serialized across processes and implementations by the
	// same lock file the JS reference uses.
	wait, onTimeoutNull := 45*time.Second, false
	if !opts.Finalize {
		wait, onTimeoutNull = 2500*time.Millisecond, true
	}
	committed := false
	err = withCommitLock(captureJSON, wait, func(stillHeld func() bool) error {
		if !opts.Finalize {
			if _, err := os.Stat(spool); err != nil {
				return nil // finalizer won — discard silently
			}
			if jst, err := os.Stat(captureJSON); err == nil {
				if jsonBefore == nil || !jst.ModTime().Equal(*jsonBefore) {
					return nil
				}
			} else if jsonBefore != nil {
				return nil
			}
			if !stillHeld() {
				return nil
			}
			if err := os.Rename(tmp, captureJSON); err != nil {
				return err
			}
			committed = true
			// Keep the freshness gate honest: stamp with the spool's mtime.
			os.Chtimes(captureJSON, time.Now(), spoolMtime)
			return nil
		}
		nst, err := os.Stat(spool)
		if err != nil || nst.Size() != spoolSizeAtStart {
			return ErrCommitRetry
		}
		if !stillHeld() {
			return ErrCommitRetry
		}
		if err := os.Rename(tmp, captureJSON); err != nil {
			return err
		}
		committed = true
		os.Remove(spool)
		os.Remove(PidPath(captureJSON))
		return nil
	})
	if err != nil {
		os.Remove(tmp)
		if errors.Is(err, ErrLockStuck) && onTimeoutNull {
			return 0, os.ErrNotExist // snapshots give up quietly
		}
		return 0, err
	}
	if !committed {
		os.Remove(tmp)
		return 0, os.ErrNotExist // snapshot discarded (finalizer won)
	}
	return spans, nil
}

// writeHeader emits everything before the span lines: the skeleton minus
// its spans, then the root span — stamped in finalize mode.
func writeHeader(w *bufio.Writer, skeleton map[string]json.RawMessage, opts AssembleOptions) error {
	var rootSpans []json.RawMessage
	if skeleton["spans"] != nil {
		json.Unmarshal(skeleton["spans"], &rootSpans)
	}
	root := map[string]json.RawMessage{}
	if len(rootSpans) > 0 {
		json.Unmarshal(rootSpans[0], &root)
	} else {
		root["id"] = json.RawMessage(`"root"`)
		root["parent_id"] = json.RawMessage(`null`)
		root["type"] = json.RawMessage(`"agent"`)
	}
	head := map[string]json.RawMessage{}
	for k, v := range skeleton {
		if k != "spans" {
			head[k] = v
		}
	}
	if opts.Finalize {
		endedAt := opts.EndedAt
		if endedAt == "" {
			endedAt = time.Now().UTC().Format("2006-01-02T15:04:05.000Z")
		}
		root["ended_at"] = json.RawMessage(`"` + endedAt + `"`)
		root["status"] = json.RawMessage(`"ok"`)
		if head["metadata"] != nil {
			var md map[string]json.RawMessage
			if json.Unmarshal(head["metadata"], &md) == nil {
				delete(md, "in_progress")
				b, _ := json.Marshal(md)
				head["metadata"] = b
			}
		}
	}
	headJSON, err := json.Marshal(head)
	if err != nil {
		return err
	}
	rootJSON, err := json.Marshal(root)
	if err != nil {
		return err
	}
	glue := `,"spans":[`
	if string(headJSON) == "{}" {
		glue = `"spans":[`
	}
	w.Write(headJSON[:len(headJSON)-1])
	w.WriteString(glue)
	w.Write(rootJSON)
	return nil
}

// withCommitLock: O_EXCL lock with token, grave-rename break at 30s,
// stillHeld verification — the protocol's commit mutex.
func withCommitLock(captureJSON string, wait time.Duration, fn func(stillHeld func() bool) error) error {
	lock := LockPath(captureJSON)
	token := fmt.Sprintf("%d:%s", os.Getpid(), randHex(6))
	deadline := time.Now().Add(wait)
	for {
		f, err := os.OpenFile(lock, os.O_CREATE|os.O_EXCL|os.O_WRONLY, 0o644)
		if err == nil {
			f.WriteString(token)
			f.Close()
			stillHeld := func() bool {
				b, err := os.ReadFile(lock)
				return err == nil && string(b) == token
			}
			ferr := fn(stillHeld)
			if stillHeld() {
				os.Remove(lock)
			}
			return ferr
		}
		if !errors.Is(err, os.ErrExist) {
			return err
		}
		if st, serr := os.Stat(lock); serr == nil && time.Since(st.ModTime()) > LockBreakAge {
			// Break by rename: atomic, exactly one breaker wins.
			grave := fmt.Sprintf("%s.%d.%s.stale", lock, os.Getpid(), randHex(3))
			if os.Rename(lock, grave) == nil {
				os.Remove(grave)
			}
			continue
		}
		if time.Now().After(deadline) {
			return ErrLockStuck
		}
		time.Sleep(50 * time.Millisecond)
	}
}

func atomicWrite(path string, body []byte) error {
	tmp := fmt.Sprintf("%s.%s.tmp", path, randHex(4))
	if err := os.WriteFile(tmp, body, 0o644); err != nil {
		return err
	}
	return os.Rename(tmp, path)
}

func randHex(n int) string {
	b := make([]byte, n)
	rand.Read(b)
	return hex.EncodeToString(b)
}

func absDiff(a, b int64) int64 {
	if a > b {
		return a - b
	}
	return b - a
}
