package handoff

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/lftherios/session-link/internal/importers"
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

func TestSaveReusesSnapshotWhileTranscriptIsUnchanged(t *testing.T) {
	transcript, err := os.ReadFile("../../../testdata/import/claude-code/basic/input.session.jsonl")
	if err != nil {
		t.Fatal(err)
	}
	root := t.TempDir()
	file := filepath.Join(root, "claude", "-work-project", "session.jsonl")
	if err := os.MkdirAll(filepath.Dir(file), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(file, transcript, 0o600); err != nil {
		t.Fatal(err)
	}
	candidates, err := importers.Locations{Claude: filepath.Join(root, "claude")}.Recent("claude-code", "/work/project", "", 5)
	if err != nil || len(candidates) != 1 {
		t.Fatalf("%v %v", candidates, err)
	}
	source := Native(candidates[0])
	if !slices.Equal(source.Files, []string{file}) {
		t.Fatalf("transcript not tracked: %v", source.Files)
	}
	reads, read := 0, source.Document
	source.Document = func() (map[string]any, error) { reads++; return read() }
	dir := t.TempDir()
	save := func(wantReads int) string {
		t.Helper()
		id, err := Save(dir, source)
		if err != nil {
			t.Fatal(err)
		}
		if reads != wantReads {
			t.Fatalf("imported %d times, want %d", reads, wantReads)
		}
		return id
	}
	first := save(1)
	if again := save(1); again != first {
		t.Fatalf("unchanged transcript gave %s, then %s", first, again)
	}
	// The agent keeps writing, so the grown transcript becomes a new snapshot.
	grown := append(slices.Clone(transcript), []byte(`{"type":"user","uuid":"later","timestamp":"2026-09-11T10:00:00Z","message":{"role":"user","content":"One more request"}}`+"\n")...)
	if err := os.WriteFile(file, grown, 0o600); err != nil {
		t.Fatal(err)
	}
	second := save(2)
	if second == first {
		t.Fatal("new work was not saved")
	}
	if again := save(2); again != second {
		t.Fatal("grown transcript was not reused")
	}
	// A damaged or missing snapshot is rebuilt rather than served.
	if err := os.WriteFile(filepath.Join(dir, second+".json"), []byte("{}"), 0o600); err != nil {
		t.Fatal(err)
	}
	if again := save(3); again != second {
		t.Fatal("rebuilt snapshot changed")
	}
	if err := os.Remove(filepath.Join(dir, second+".json")); err != nil {
		t.Fatal(err)
	}
	save(4)
	// Another build of slink never trusts an import this one made.
	previous := buildStamp
	buildStamp = "another build"
	t.Cleanup(func() { buildStamp = previous })
	save(5)
	save(5)
}

// heavySession is a valid session whose tool call recorded a large result.
func heavySession(t *testing.T) ([]byte, string) {
	t.Helper()
	raw, err := os.ReadFile("../../../testdata/import/pi/basic/golden.run.json")
	if err != nil {
		t.Fatal(err)
	}
	var doc map[string]any
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatal(err)
	}
	marker := "recorded-output-marker"
	spans, _ := doc["spans"].([]any)
	for _, value := range spans {
		span, _ := value.(map[string]any)
		if span["type"] != "tool_call" {
			continue
		}
		output, _ := span["output"].(map[string]any)
		if output == nil {
			output = map[string]any{}
			span["output"] = output
		}
		output["result"] = marker + strings.Repeat("x", 900<<10)
		data, err := json.Marshal(doc)
		if err != nil {
			t.Fatal(err)
		}
		return data, marker
	}
	t.Fatal("fixture has no tool call to weigh down")
	return nil, ""
}

func TestSaveWritesAReadingCopyWithoutRecordedOutput(t *testing.T) {
	data, marker := heavySession(t)
	dir := t.TempDir()
	id, err := Save(dir, Source{Read: func() ([]byte, error) { return data, nil }})
	if err != nil {
		t.Fatal(err)
	}
	whole, err := os.ReadFile(filepath.Join(dir, id+".json"))
	if err != nil || !bytes.Contains(whole, []byte(marker)) {
		t.Fatal("the saved session lost its recorded output")
	}
	reading, err := os.ReadFile(filepath.Join(dir, id+ReadingSuffix))
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(reading, []byte(marker)) {
		t.Fatal("the reading copy still carries recorded output")
	}
	if len(reading)*4 > len(whole) {
		t.Fatalf("reading copy is %d bytes against %d", len(reading), len(whole))
	}
	var light, full map[string]any
	if json.Unmarshal(reading, &light) != nil || json.Unmarshal(whole, &full) != nil {
		t.Fatal("copies are not both JSON")
	}
	if len(light["spans"].([]any)) != len(full["spans"].([]any)) {
		t.Fatal("the reading copy lost spans")
	}
	omitted := 0
	for _, value := range light["spans"].([]any) {
		span, _ := value.(map[string]any)
		output, _ := span["output"].(map[string]any)
		if output["result_omitted"] == true && output["result"] == nil {
			omitted++
		}
	}
	if omitted == 0 {
		t.Fatal("nothing is marked as left out")
	}
	// A session with no weight to shed keeps one copy.
	light2, err := os.ReadFile("../../../testdata/import/pi/basic/golden.run.json")
	if err != nil {
		t.Fatal(err)
	}
	plain, err := Save(dir, Source{Read: func() ([]byte, error) { return light2, nil }})
	if err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, plain+ReadingSuffix)); !os.IsNotExist(err) {
		t.Fatal("wrote a reading copy that saves nothing")
	}
}

// A session's preview is made from its sub-agents' transcripts too, so one of
// them changing means the preview is made again.
func TestSaveFollowsASubAgentsTranscript(t *testing.T) {
	fixture := "../../../testdata/import/claude-code/sub-agent"
	root := t.TempDir()
	session := filepath.Join(root, "claude", "-work-checks", "session.jsonl")
	agents := filepath.Join(root, "claude", "-work-checks", "session", "subagents")
	if err := os.MkdirAll(agents, 0o700); err != nil {
		t.Fatal(err)
	}
	copyFile := func(from, to string) {
		t.Helper()
		data, err := os.ReadFile(from)
		if err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(to, data, 0o600); err != nil {
			t.Fatal(err)
		}
	}
	copyFile(filepath.Join(fixture, "input.session.jsonl"), session)
	recorded, _ := filepath.Glob(filepath.Join(fixture, "input.session", "subagents", "*"))
	for _, file := range recorded {
		copyFile(file, filepath.Join(agents, filepath.Base(file)))
	}
	transcripts, _ := filepath.Glob(filepath.Join(agents, "*.jsonl"))

	candidates, err := importers.Locations{Claude: filepath.Join(root, "claude")}.Recent("claude-code", "/work/checks", "", 5)
	if err != nil || len(candidates) != 1 {
		t.Fatalf("%v %v", candidates, err)
	}
	source := Native(candidates[0])
	if !slices.Equal(source.Files, append([]string{session}, transcripts...)) {
		t.Fatalf("the sub-agent's transcript is not tracked: %v", source.Files)
	}
	dir := t.TempDir()
	first, err := Save(dir, source)
	if err != nil {
		t.Fatal(err)
	}
	if again, _ := Save(dir, source); again != first {
		t.Fatal("an unchanged session was imported again")
	}
	// The sub-agent is still working: its transcript grows, its parent's does not.
	extra := `{"type":"assistant","isSidechain":true,"timestamp":"2026-10-08T07:00:59.000Z","message":{"id":"msg_later","role":"assistant","model":"claude-opus-5-5","content":[{"type":"text","text":"One more finding."}]}}` + "\n"
	f, err := os.OpenFile(transcripts[0], os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		t.Fatal(err)
	}
	f.WriteString(extra)
	f.Close()
	later, err := Save(dir, source)
	if err != nil {
		t.Fatal(err)
	}
	if later == first {
		t.Fatal("the preview kept the sub-agent's earlier work after its transcript changed")
	}
	saved, _ := os.ReadFile(filepath.Join(dir, later+".json"))
	if !strings.Contains(string(saved), "One more finding.") {
		t.Fatal("the new preview lacks the sub-agent's later work")
	}
}
