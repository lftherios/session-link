package open

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/lftherios/session-link/internal/handoff"
)

func TestSessionPickerUsesQuietHeaderAndDayGroups(t *testing.T) {
	now := time.Now()
	s := &Server{Project: "/work/project", PreviewDir: t.TempDir(), Sources: []handoff.Source{
		{ID: "abcdef12-3456-7890", Harness: "claude-code", Name: "Fix <b>header</b>", Prompt: "Please fix the header", Updated: now.Add(-5 * time.Minute)},
		{ID: "older", Harness: "codex", Prompt: "Review the project status", Updated: now.AddDate(0, 0, -1), Started: time.Date(2026, 9, 10, 9, 0, 0, 0, time.Local)},
		{ID: "renamed", Harness: "pi", Name: "rollout-2026.jsonl", Prompt: "Compare competitors", Updated: now.Add(-time.Hour)},
	}}
	if err := atomicJSON(filepath.Join(s.draftsDir(), "titles", sessionTitleKey("pi", "renamed")+".json"), map[string]string{"title": "Competitor comparison"}); err != nil {
		t.Fatal(err)
	}
	body := action(s, "GET", "/", "", "").Body.String()
	for _, want := range []string{`<h1 title="/work/project">project</h1>`, `aria-label="Today"`, `aria-label="Yesterday"`,
		`<span class="row-title">Fix &lt;b&gt;header&lt;/b&gt;</span>`, `<span class="row-title untitled">Untitled · Codex · Sep 10, 2026</span>`,
		`<span class="row-title">Competitor comparison</span>`, `<span class="row-preview">Review the project status</span>`,
		`<code title="abcdef12-3456-7890">abcdef12</code>`, `placeholder="Search 3 sessions…"`, `id="stop"`} {
		if !strings.Contains(body, want) {
			t.Errorf("picker is missing %q", want)
		}
	}
	for _, gone := range []string{"local sessions", "Bring your session", "Project:", "Open preview", `class="row-title">rollout-2026.jsonl`} {
		if strings.Contains(body, gone) {
			t.Errorf("picker still shows %q", gone)
		}
	}
}

func TestCapturesIndexUsesTheSameList(t *testing.T) {
	dir := t.TempDir()
	data, err := os.ReadFile("../../../testdata/import/pi/basic/golden.run.json")
	if err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "capture.json"), data, 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(filepath.Join(dir, "broken.json"), []byte("{"), 0o600); err != nil {
		t.Fatal(err)
	}
	body := action(&Server{CaptureDir: dir}, "GET", "/", "", "").Body.String()
	for _, want := range []string{"<h1>Captured sessions</h1>", `class="row" href="/r/capture"`, `class="row dead"`, "can&#39;t render — invalid JSON", `placeholder="Search 2 captures…"`} {
		if !strings.Contains(body, want) {
			t.Errorf("captures index is missing %q", want)
		}
	}
	if strings.Contains(body, "nothing here has left your machine") {
		t.Error("captures index still shows the old eyebrow")
	}
}

func TestDayLabels(t *testing.T) {
	now := time.Date(2026, 9, 11, 15, 0, 0, 0, time.Local)
	for _, c := range []struct {
		at   time.Time
		want string
	}{
		{now.Add(-2 * time.Hour), "Today"},
		{now.AddDate(0, 0, -1), "Yesterday"},
		{now.AddDate(0, 0, -3), now.AddDate(0, 0, -3).Format("Monday")},
		{time.Date(2026, 8, 1, 9, 0, 0, 0, time.Local), "August 1"},
		{time.Date(2025, 12, 24, 9, 0, 0, 0, time.Local), "December 24, 2025"},
		{time.Time{}, "Undated"},
	} {
		if got := dayLabel(c.at, now); got != c.want {
			t.Errorf("dayLabel(%v) = %q, want %q", c.at, got, c.want)
		}
	}
}
