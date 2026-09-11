package importers

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestTranscriptTitlesPreferHarnessTitlesOverPrompts(t *testing.T) {
	file := filepath.Join(t.TempDir(), "session.jsonl")
	lines := []string{
		`{"type":"user","timestamp":"2026-09-11T11:50:16.104Z","message":{"role":"user","content":"Fix the header"}}`,
		`{"type":"ai-title","aiTitle":"Header work","sessionId":"s"}`,
		`{"type":"assistant","timestamp":"2026-09-11T11:51:00Z","message":{"role":"assistant","content":[{"type":"text","text":"On it"}]}}`,
		`{"type":"ai-title","aiTitle":"Viewer header redesign","sessionId":"s"}`,
	}
	if err := os.WriteFile(file, []byte(strings.Join(lines, "\n")+"\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	_, name, prompt, started := peekTitles(file, 40)
	if name != "Viewer header redesign" || prompt != "Fix the header" || started.UTC().Format("2006-01-02T15:04") != "2026-09-11T11:50" {
		t.Fatalf("name=%q prompt=%q started=%v", name, prompt, started)
	}
	if latest := latestAITitle(file); latest != "Viewer header redesign" {
		t.Fatalf("latest title = %q", latest)
	}
	if _, title := peekTranscript(file, 40); title != "Viewer header redesign" {
		t.Fatalf("title = %q", title)
	}
	plain := filepath.Join(t.TempDir(), "plain.jsonl")
	os.WriteFile(plain, []byte(lines[0]+"\n"), 0o600)
	if _, name, prompt, _ := peekTitles(plain, 40); name != "" || prompt != "Fix the header" || latestAITitle(plain) != "" {
		t.Fatalf("untitled transcript: name=%q prompt=%q", name, prompt)
	}
}
