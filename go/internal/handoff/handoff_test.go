package handoff

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"testing"

	"github.com/lftherios/session-link/internal/spool"
)

func TestFilePreviewPreservesSourceAndEvidence(t *testing.T) {
	for _, harness := range []string{"claude-code", "pi", "codex"} {
		t.Run(harness, func(t *testing.T) {
			original, err := os.ReadFile(filepath.Join("../../../testdata/import", harness, "basic/input.session.jsonl"))
			if err != nil {
				t.Fatal(err)
			}
			file := filepath.Join(t.TempDir(), "external transcript.jsonl")
			if err := os.WriteFile(file, original, 0o600); err != nil {
				t.Fatal(err)
			}
			source, err := File(file, "")
			if err != nil {
				t.Fatal(err)
			}
			dir := t.TempDir()
			id, err := Save(dir, source)
			if err != nil {
				t.Fatal(err)
			}
			preview, err := os.ReadFile(filepath.Join(dir, id+".json"))
			if err != nil {
				t.Fatal(err)
			}
			var run map[string]any
			if err := json.Unmarshal(preview, &run); err != nil {
				t.Fatal(err)
			}
			if len(run["spans"].([]any)) < 2 {
				t.Fatal("messages or tool evidence lost")
			}
			again, err := Save(dir, source)
			if err != nil || again != id {
				t.Fatalf("identical content changed ID: %s %s %v", id, again, err)
			}
			untouched, _ := os.ReadFile(file)
			if !bytes.Equal(original, untouched) {
				t.Fatal("preview changed source")
			}
			if err := os.WriteFile(file, []byte("changed by the agent"), 0o600); err != nil {
				t.Fatal(err)
			}
			saved, _ := os.ReadFile(filepath.Join(dir, id+".json"))
			if !bytes.Equal(saved, preview) {
				t.Fatal("saved preview changed with source")
			}
			if _, err := Save(dir, source); err == nil {
				t.Fatal("unreadable source accepted")
			}
		})
	}
}

func TestInvalidPreviewDoesNotWrite(t *testing.T) {
	dir := t.TempDir()
	for _, doc := range []string{`null`, `{}`, `{"schema":"session/v0","spans":[]}`, `not JSON`} {
		_, err := Save(dir, Source{Read: func() ([]byte, error) { return []byte(doc), nil }})
		if err == nil {
			t.Fatalf("accepted %s", doc)
		}
	}
	files, _ := os.ReadDir(dir)
	if len(files) != 0 {
		t.Fatal("invalid preview left files behind")
	}
}

func TestPreviewDuringLiveRecording(t *testing.T) {
	data, err := os.ReadFile("../../../testdata/spool/snapshot-in-progress/input.spool")
	if err != nil {
		t.Fatal(err)
	}
	file := filepath.Join(t.TempDir(), "live.json")
	if err := os.WriteFile(spool.SpoolPath(file), data, 0o600); err != nil {
		t.Fatal(err)
	}
	source, err := File(file, "")
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	id, err := Save(dir, source)
	if err != nil {
		t.Fatal(err)
	}
	before, err := os.ReadFile(filepath.Join(dir, id+".json"))
	if err != nil {
		t.Fatal(err)
	}
	if !bytes.Contains(before, []byte(`"in_progress":true`)) {
		t.Fatal("recording status lost")
	}
	data = append(data, []byte("\n"+`{"id":"later","parent_id":"root","type":"custom","name":"Continued work"}`+"\n")...)
	if err := os.WriteFile(spool.SpoolPath(file), data, 0o600); err != nil {
		t.Fatal(err)
	}
	next, err := Save(dir, source)
	if err != nil || next == id {
		t.Fatalf("new work not saved independently: %s %v", next, err)
	}
	after, _ := os.ReadFile(filepath.Join(dir, id+".json"))
	if !bytes.Equal(before, after) {
		t.Fatal("live work changed the earlier preview")
	}
}
