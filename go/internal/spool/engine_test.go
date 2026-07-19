package spool

import (
	"encoding/json"
	"errors"
	"os"
	"path/filepath"
	"reflect"
	"testing"
	"time"
)

// repoRoot walks up from the package dir to the repo checkout.
func repoRoot(t *testing.T) string {
	t.Helper()
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	return filepath.Join(wd, "..", "..", "..")
}

// TestGoldenSpools runs the same fixtures the JS reference pins
// (scripts/golden.mjs, testdata/spool/*): parity is PARSED equality of
// the assembled document.
func TestGoldenSpools(t *testing.T) {
	casesDir := filepath.Join(repoRoot(t), "testdata", "spool")
	entries, err := os.ReadDir(casesDir)
	if err != nil {
		t.Fatalf("no spool fixtures: %v", err)
	}
	ran := 0
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		ran++
		t.Run(e.Name(), func(t *testing.T) {
			dir := filepath.Join(casesDir, e.Name())
			var meta struct {
				Mode    string `json:"mode"`
				EndedAt string `json:"endedAt"`
			}
			mustReadJSON(t, filepath.Join(dir, "input.meta.json"), &meta)
			var golden struct {
				Spans *int           `json:"spans"`
				Doc   map[string]any `json:"doc"`
			}
			mustReadJSON(t, filepath.Join(dir, "golden.doc.json"), &golden)

			cap := filepath.Join(t.TempDir(), "cap.json")
			spoolBytes, err := os.ReadFile(filepath.Join(dir, "input.spool"))
			if err != nil {
				t.Fatal(err)
			}
			if err := os.WriteFile(SpoolPath(cap), spoolBytes, 0o644); err != nil {
				t.Fatal(err)
			}

			n, err := Assemble(cap, AssembleOptions{Finalize: meta.Mode == "finalize", EndedAt: meta.EndedAt})
			if golden.Spans == nil {
				// The JS reference returned null → refused/empty.
				if err == nil {
					t.Fatalf("expected refusal, assembled %d spans", n)
				}
				if _, statErr := os.Stat(cap); statErr == nil {
					t.Fatal("refused assembly must not write the capture")
				}
				return
			}
			if err != nil {
				t.Fatalf("assemble: %v", err)
			}
			if n != *golden.Spans {
				t.Fatalf("span count: got %d want %d", n, *golden.Spans)
			}
			var got map[string]any
			mustReadJSON(t, cap, &got)
			if !reflect.DeepEqual(got, golden.Doc) {
				g, _ := json.MarshalIndent(got, "", "  ")
				w, _ := json.MarshalIndent(golden.Doc, "", "  ")
				t.Fatalf("document mismatch\n--- got ---\n%s\n--- want ---\n%s", g, w)
			}
		})
	}
	if ran == 0 {
		t.Fatal("zero golden fixtures ran — the parity test passed vacuously")
	}
}

func TestAppendAssembleRoundTripWithSeparators(t *testing.T) {
	cap := filepath.Join(t.TempDir(), "cap.json")
	skeleton := map[string]any{
		"schema": "session/v0", "name": "t",
		"created_at": "2026-07-19T09:00:00.000Z",
		"metadata":   map[string]any{"in_progress": true},
		"spans": []any{map[string]any{
			"id": "root", "parent_id": nil, "type": "agent",
			"started_at": "2026-07-19T09:00:00.000Z",
		}},
	}
	text := "a\u2028b\u2029c"
	span := map[string]any{
		"id": "s1", "parent_id": "root", "type": "llm_call",
		"model": map[string]any{"id": "m"},
		"input": map[string]any{"messages": []any{map[string]any{
			"role": "user", "content": []any{map[string]any{"type": "text", "text": text}},
		}}},
		"output":     map[string]any{"messages": []any{}},
		"started_at": "2026-07-19T09:00:01.000Z", "ended_at": "2026-07-19T09:00:02.000Z",
	}
	sk, _ := EncodeLine(skeleton)
	sp, _ := EncodeLine(span)
	if err := Append(cap, [][]byte{sp}, sk, nil); err != nil {
		t.Fatal(err)
	}
	// The escaping is the point: the raw separators must not appear in the file.
	raw, _ := os.ReadFile(SpoolPath(cap))
	if containsRune(raw, '\u2028') || containsRune(raw, '\u2029') {
		t.Fatal("raw separators leaked into the spool")
	}
	n, err := Assemble(cap, AssembleOptions{Finalize: true, EndedAt: "2026-07-19T09:30:00.000Z"})
	if err != nil || n != 1 {
		t.Fatalf("assemble: n=%d err=%v", n, err)
	}
	var doc struct {
		Spans []struct {
			Input *struct {
				Messages []struct {
					Content []struct{ Text string }
				}
			}
		}
	}
	mustReadJSON(t, cap, &doc)
	if doc.Spans[1].Input.Messages[0].Content[0].Text != text {
		t.Fatal("separator text did not round-trip")
	}
}

func TestAppendClosedGateAndHeaderRecreation(t *testing.T) {
	cap := filepath.Join(t.TempDir(), "cap.json")
	sk, _ := EncodeLine(map[string]any{"schema": "session/v0", "spans": []any{map[string]any{"id": "root", "type": "agent"}}})
	s1, _ := EncodeLine(map[string]any{"id": "s1", "type": "custom"})
	if err := Append(cap, [][]byte{s1}, sk, func() bool { return true }); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(SpoolPath(cap)); err == nil {
		t.Fatal("closed gate must drop the write entirely")
	}
	if err := Append(cap, [][]byte{s1}, sk, nil); err != nil {
		t.Fatal(err)
	}
	if _, err := Assemble(cap, AssembleOptions{Finalize: true, EndedAt: "2026-07-19T09:30:00.000Z"}); err != nil {
		t.Fatal(err)
	}
	// A late append after finalize recreates the spool WITH a header.
	if err := Append(cap, [][]byte{s1}, sk, nil); err != nil {
		t.Fatal(err)
	}
	n, err := Assemble(cap, AssembleOptions{Finalize: true, EndedAt: "2026-07-19T09:31:00.000Z"})
	if err != nil || n != 1 {
		t.Fatalf("recreated spool must be self-describing: n=%d err=%v", n, err)
	}
}

func TestOwnerLivenessRule(t *testing.T) {
	cap := filepath.Join(t.TempDir(), "cap.json")
	sk, _ := EncodeLine(map[string]any{"schema": "session/v0", "spans": []any{}})
	s1, _ := EncodeLine(map[string]any{"id": "s1", "type": "custom"})
	if err := Append(cap, [][]byte{s1}, sk, nil); err != nil {
		t.Fatal(err)
	}
	if !OwnerAlive(cap) {
		t.Fatal("own live pid with fresh sidecar must be alive")
	}
	// Boot mismatch + fresh sidecar: alive (stepped clock case).
	writeSidecar(t, cap, OwnerSidecar{Pid: os.Getpid(), Boot: 12345})
	if !OwnerAlive(cap) {
		t.Fatal("fresh heartbeat must carry a boot mismatch")
	}
	// Boot mismatch + stale sidecar: dead (recycled pid after reboot).
	staleSidecar(t, cap)
	if OwnerAlive(cap) {
		t.Fatal("stale + boot mismatch must be dead")
	}
	// pid 1: alive only via freshness.
	writeSidecar(t, cap, OwnerSidecar{Pid: 1, Boot: bootMs()})
	if !OwnerAlive(cap) {
		t.Fatal("fresh pid-1 sidecar is alive")
	}
	staleSidecar(t, cap)
	if OwnerAlive(cap) {
		t.Fatal("stale pid-1 must not ride the boot branch")
	}
	// Dead pid.
	writeSidecar(t, cap, OwnerSidecar{Pid: 999999, Boot: bootMs()})
	if OwnerAlive(cap) {
		t.Fatal("dead pid is dead")
	}
	// Release.
	writeSidecar(t, cap, OwnerSidecar{Pid: os.Getpid(), Boot: bootMs()})
	if err := ReleaseOwnership(cap); err != nil {
		t.Fatal(err)
	}
	if OwnerAlive(cap) {
		t.Fatal("released ownership must read dead")
	}
}

func TestStaleLockBreak(t *testing.T) {
	cap := filepath.Join(t.TempDir(), "cap.json")
	sk, _ := EncodeLine(map[string]any{"schema": "session/v0", "spans": []any{}})
	s1, _ := EncodeLine(map[string]any{"id": "s1", "type": "custom"})
	if err := Append(cap, [][]byte{s1}, sk, nil); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(LockPath(cap), []byte("999999:dead"), 0o644); err != nil {
		t.Fatal(err)
	}
	old := time.Now().Add(-time.Minute)
	if err := os.Chtimes(LockPath(cap), old, old); err != nil {
		t.Fatal(err)
	}
	n, err := Assemble(cap, AssembleOptions{Finalize: true, EndedAt: "2026-07-19T09:30:00.000Z"})
	if err != nil || n != 1 {
		t.Fatalf("stale lock must be broken: n=%d err=%v", n, err)
	}
	if _, err := os.Stat(LockPath(cap)); err == nil {
		t.Fatal("lock must be cleaned up")
	}
}

func TestFreshStuckLockTimesOut(t *testing.T) {
	cap := filepath.Join(t.TempDir(), "cap.json")
	sk, _ := EncodeLine(map[string]any{"schema": "session/v0", "spans": []any{}})
	s1, _ := EncodeLine(map[string]any{"id": "s1", "type": "custom"})
	if err := Append(cap, [][]byte{s1}, sk, nil); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(LockPath(cap), []byte("999999:fresh"), 0o644); err != nil {
		t.Fatal(err)
	}
	// Snapshot mode: short budget, quiet give-up.
	_, err := Assemble(cap, AssembleOptions{})
	if !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("snapshot behind a fresh stuck lock must give up quietly, got %v", err)
	}
}

/* ------------------------------------------------------------- helpers */

func mustReadJSON(t *testing.T, path string, v any) {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(b, v); err != nil {
		t.Fatalf("%s: %v", path, err)
	}
}

func writeSidecar(t *testing.T, cap string, sc OwnerSidecar) {
	t.Helper()
	b, _ := json.Marshal(sc)
	if err := os.WriteFile(PidPath(cap), b, 0o644); err != nil {
		t.Fatal(err)
	}
}

func staleSidecar(t *testing.T, cap string) {
	t.Helper()
	old := time.Now().Add(-10 * time.Minute)
	if err := os.Chtimes(PidPath(cap), old, old); err != nil {
		t.Fatal(err)
	}
}

func containsRune(b []byte, r rune) bool {
	for _, c := range string(b) {
		if c == r {
			return true
		}
	}
	return false
}
