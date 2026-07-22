package importers

import (
	"os"
	"path/filepath"
	"testing"
	"time"
)

func TestNotFoundIDErrors(t *testing.T) {
	// No DB present → openDB fails; a bogus id must surface an error, never
	// a silent empty Input (which built a degenerate capture).
	t.Setenv("HERMES_STATE_DB", filepath.Join(t.TempDir(), "nope.db"))
	if _, err := loadHermes("bogus"); err == nil {
		t.Fatal("loadHermes on a missing DB must error")
	}
}

func TestSniffWidenedForCodex(t *testing.T) {
	// A codex rollout whose head lines are response_items (late/stripped
	// session_meta) must still sniff as codex, like JS looksLikeCodex.
	lines := []string{
		`{"type":"response_item","payload":{"type":"message","role":"user","content":[]}}`,
		`{"type":"turn_context","payload":{"model":"gpt-5"}}`,
	}
	if got := sniff(lines); got != "codex" {
		t.Fatalf("sniff widened predicate: got %q want codex", got)
	}
}

func TestAutoDetectPicksMostRecent(t *testing.T) {
	// Two harnesses' sessions in one cwd: the NEWEST wins, not a fixed order.
	home := t.TempDir()
	t.Setenv("HOME", home)
	cwd := "/work/proj"
	// An older claude-code session...
	ccDir := ClaudeProjectDir(cwd)
	os.MkdirAll(ccDir, 0o755)
	ccFile := filepath.Join(ccDir, "old.jsonl")
	os.WriteFile(ccFile, []byte(`{"type":"user","message":{"role":"user","content":"x"}}`+"\n"), 0o644)
	old := timeAgo(2)
	os.Chtimes(ccFile, old, old)
	// ...and a newer pi session.
	piDir := piProjectDir(cwd)
	os.MkdirAll(piDir, 0o755)
	piFile := filepath.Join(piDir, "new.jsonl")
	os.WriteFile(piFile, []byte(`{"type":"session"}`+"\n"), 0o644)

	found, ok := Latest("", cwd)
	if !ok {
		t.Fatal("expected a session")
	}
	if found.Harness != "pi" {
		t.Fatalf("auto-detect must pick the newest across harnesses: got %s, want pi", found.Harness)
	}
}

func TestNewestAnywhere(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	t.Setenv("CODEX_HOME", filepath.Join(home, "no-codex"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(home, "no-xdg"))
	t.Setenv("HERMES_STATE_DB", filepath.Join(home, "no.db"))

	if _, ok := NewestAnywhere(""); ok {
		t.Fatal("empty machine reported a session")
	}

	older := filepath.Join(home, ".claude", "projects", "-proj-a")
	newer := filepath.Join(home, ".claude", "projects", "-proj-b")
	os.MkdirAll(older, 0o755)
	os.MkdirAll(newer, 0o755)
	line := `{"type":"user","cwd":"/proj/b","message":{"role":"user","content":"Ship the empty-state UX for import"}}` + "\n"
	os.WriteFile(filepath.Join(older, "old.jsonl"), []byte(line), 0o644)
	os.WriteFile(filepath.Join(newer, "new.jsonl"), []byte(line), 0o644)
	past := time.Now().Add(-2 * time.Hour)
	os.Chtimes(filepath.Join(older, "old.jsonl"), past, past)

	e, ok := NewestAnywhere("")
	if !ok {
		t.Fatal("found nothing")
	}
	if e.Harness != "claude-code" || filepath.Base(e.File) != "new.jsonl" {
		t.Fatalf("picked %s %s, want claude-code new.jsonl", e.Harness, e.File)
	}
	if e.Dir != "/proj/b" {
		t.Fatalf("cwd = %q, want /proj/b (peeked from the transcript)", e.Dir)
	}
	if e.Title != "Ship the empty-state UX for import" {
		t.Fatalf("title = %q", e.Title)
	}
	if _, ok := NewestAnywhere("pi"); ok {
		t.Fatal("pi reported a session on a claude-only machine")
	}
}
