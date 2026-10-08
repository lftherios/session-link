package importers

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// aiderUTC reads aider's clock times as UTC for a test, wherever it runs.
func aiderUTC(t *testing.T) {
	t.Helper()
	zone := aiderZone
	aiderZone = time.UTC
	t.Cleanup(func() { aiderZone = zone })
}

const aiderTwoRuns = `
# aider chat started at 2026-10-06 09:00:00

> Aider v0.86.2  
> Main model: anthropic/claude-sonnet with diff edit format  
> Added upload.py to the chat.  

#### Find the race in the upload test  

The retry starts before the first attempt releases its lock.

> Tokens: 2.4k sent, 38 received.  

#### /exit  

# aider chat started at 2026-10-06 10:30:00

> Aider v0.86.2  
> Model: openai/gpt-small with whole edit format  

#### /add notes.md  
> Added notes.md to the chat  

#### Write the fix up  
#### in two lines  

Done.
`

func TestAiderRunsAreTheSessionsOfOneFile(t *testing.T) {
	isolateStores(t)
	aiderUTC(t)
	project, _ := filepath.EvalSymlinks(t.TempDir())
	chat := filepath.Join(project, ".aider.chat.history.md")
	written := time.Date(2026, 10, 6, 10, 45, 0, 0, time.UTC)
	writeRecent(t, chat, aiderTwoRuns, written)
	writeRecent(t, filepath.Join(project, ".aider.input.history"), "\n# 2026-10-06 09:00:07.250000\n+Find the race in the upload test\n\n# 2026-10-06 10:30:02.000000\n+/add notes.md\n\n# 2026-10-06 10:31:05.500000\n+Write the fix up\n+in two lines\n", written)
	deeper := filepath.Join(project, "src", "upload")
	if err := os.MkdirAll(deeper, 0o700); err != nil {
		t.Fatal(err)
	}

	// Aider keeps the file at the project root; work goes on inside it.
	runs, err := Locations{}.Recent("aider", deeper, "", 30)
	if err != nil {
		t.Fatal(err)
	}
	if len(runs) != 2 || runs[0].ID != "2026-10-06T10:30:00" || runs[1].ID != "2026-10-06T09:00:00" {
		t.Fatalf("the runs of the file, newest first: %+v", runs)
	}
	// A slash command is not what the run was about.
	if runs[0].Title != "Write the fix up\nin two lines" || runs[1].Title != "Find the race in the upload test" || runs[0].Dir != project {
		t.Fatalf("titles and project: %q %q %q", runs[0].Title, runs[1].Title, runs[0].Dir)
	}
	// The last run was active until the file was last written; the one before it, until the next began.
	if runs[0].Recency != written.UnixNano() || runs[1].Recency != time.Date(2026, 10, 6, 10, 30, 0, 0, time.UTC).UnixNano() || !runs[1].Started.Equal(time.Date(2026, 10, 6, 9, 0, 0, 0, time.UTC)) {
		t.Fatalf("activity: %v %v %v", runs[0].Recency, runs[1].Recency, runs[1].Started)
	}

	in, err := runs[1].Load()
	if err != nil {
		t.Fatal(err)
	}
	earlier, err := importAider(in)
	if err != nil {
		t.Fatal(err)
	}
	metadata := m(earlier["metadata"])
	if earlier["name"] != "Find the race in the upload test" || earlier["created_at"] != "2026-10-06T09:00:00.000Z" || metadata["session_id"] != "2026-10-06T09:00:00" || metadata["cwd"] != project || metadata["harness_version"] != "0.86.2" {
		t.Fatalf("the earlier run: %v %v %+v", earlier["name"], earlier["created_at"], metadata)
	}
	spans := arr(earlier["spans"])
	// startup notices, the prompt and its reply, the token report, /exit
	if len(spans) != 5 {
		t.Fatalf("spans of the earlier run: %d", len(spans))
	}
	// What aider says before anything is typed is setup, not something it did.
	if setup := m(spans[1]); setup["type"] != "custom" || m(first(arr(m(setup["input"])["messages"])))["role"] != "system" {
		t.Fatalf("startup notices: %+v", setup)
	}
	call := m(spans[2])
	if call["type"] != "llm_call" || m(call["model"])["id"] != "anthropic/claude-sonnet" || call["started_at"] != "2026-10-06T09:00:07.250Z" {
		t.Fatalf("the reply's call: %+v", call)
	}
	if _, ended := call["ended_at"]; ended {
		t.Fatal("aider records no time for a reply, so none is given")
	}
	if text := strOr(m(first(arr(m(first(arr(m(call["output"])["messages"])))["content"])))["text"], ""); !strings.HasPrefix(text, "The retry starts") || strings.Contains(text, "Tokens") {
		t.Fatalf("reply text: %q", text)
	}

	one, err := Locations{}.Recent("aider", project, "2026-10-06T10:30:00", 30)
	if err != nil || len(one) != 1 {
		t.Fatalf("a run by the time it started: %+v %v", one, err)
	}
	in, _ = one[0].Load()
	later, _ := importAider(in)
	spans = arr(later["spans"])
	// startup, /add, its notice, then the two-line prompt and its reply
	if len(spans) != 5 || m(spans[2])["name"] != "/add notes.md" || m(spans[2])["type"] != "custom" || m(spans[2])["started_at"] != "2026-10-06T10:30:02.000Z" {
		t.Fatalf("a command to aider is kept where it was typed: %+v", spans[1:])
	}
	prompt := m(first(arr(m(m(spans[4])["input"])["messages"])))
	if m(spans[4])["started_at"] != "2026-10-06T10:31:05.500Z" || strOr(m(first(arr(prompt["content"])))["text"], "") != "Write the fix up\nin two lines" || m(m(spans[4])["model"])["id"] != "openai/gpt-small" {
		t.Fatalf("a typed entry of two lines, and the model of the second run: %+v", spans[4])
	}

	if found, ok := Latest("aider", project); !ok || found.Harness != "aider" {
		t.Fatal("the project's newest run is not found")
	}
	if _, ok := AiderRun(project, "2026-10-06T09:00:00"); !ok {
		t.Fatal("a run is not found by the time it started")
	}
	if _, ok := AiderRun(project, "2026-10-06T09:00:01"); ok {
		t.Fatal("a time no run started at names no run")
	}
}

func TestAiderMarksSayWhoALineIsFrom(t *testing.T) {
	blocks := aiderBlocks(strings.Split(strings.Join([]string{
		"> Aider v0.86.2  ",
		">  ",
		"> Repo-map: disabled  ",
		"",
		"#### first line  ",
		"####   ", // an empty line inside one typed entry
		"#### third line  ",
		"",
		"#### a second entry  ",
		"",
		"A reply.\r",
		"",
		"    indented code",
		"",
		"",
		"> Tokens: 12 sent, 3 received.  ",
	}, "\n"), "\n"))
	var got []string
	for _, block := range blocks {
		got = append(got, string(block.kind)+":"+strings.Join(block.lines, "|"))
	}
	want := []string{"n:Aider v0.86.2||Repo-map: disabled", "u:first line||third line", "u:a second entry", "a:A reply.||    indented code||", "n:Tokens: 12 sent, 3 received."}
	if strings.Join(got, "\n") != strings.Join(want, "\n") {
		t.Fatalf("blocks:\n%s\nwant:\n%s", strings.Join(got, "\n"), strings.Join(want, "\n"))
	}
}

func TestAiderTimesOnlyWhatTheInputHistoryMatches(t *testing.T) {
	aiderUTC(t)
	dir := t.TempDir()
	chat := filepath.Join(dir, "input.chat.history.md")
	body := "\n# aider chat started at 2026-10-06 09:00:00\n\n#### again  \n\nOne.\n\n#### again  \n\nTwo.\n\n#### reworded later  \n\nThree.\n"
	os.WriteFile(chat, []byte(body), 0o600)
	// An entry from before the run, the same text typed twice, and one the chat words differently.
	os.WriteFile(filepath.Join(dir, "input.input.history"), []byte("\n# 2026-10-05 08:00:00.000000\n+again\n\n# 2026-10-06 09:00:10.000000\n+again\n\n# 2026-10-06 09:00:20.000000\n+again\n\n# 2026-10-06 09:00:30.000000\n+reworded\n"), 0o600)
	in, harness, err := LoadFile(chat)
	if err != nil || harness != "aider" {
		t.Fatalf("an aider history given by path: %q %v", harness, err)
	}
	if _, recorded := in.Session["cwd"]; recorded {
		t.Fatal("a history that is not where aider keeps it says nothing of its project")
	}
	run, err := importAider(in)
	if err != nil {
		t.Fatal(err)
	}
	var starts []any
	for _, span := range arr(run["spans"])[1:] {
		starts = append(starts, m(span)["started_at"])
	}
	if len(starts) != 3 || starts[0] != "2026-10-06T09:00:10.000Z" || starts[1] != "2026-10-06T09:00:20.000Z" || starts[2] != nil {
		t.Fatalf("times of the three prompts: %v", starts)
	}
}

func TestLoadFileTellsAnAiderHistoryFromOtherText(t *testing.T) {
	dir := t.TempDir()
	renamed := filepath.Join(dir, "yesterday.md")
	os.WriteFile(renamed, []byte("\n# aider chat started at 2026-10-06 09:00:00\n\n#### hi  \n\nHello.\n"), 0o600)
	if _, harness, err := LoadFile(renamed); err != nil || harness != "aider" {
		t.Fatalf("a renamed aider history: %q %v", harness, err)
	}
	notes := filepath.Join(dir, "notes.md")
	os.WriteFile(notes, []byte("# Notes\n\n> a quote\n"), 0o600)
	if _, harness, _ := LoadFile(notes); harness != "" {
		t.Fatalf("a Markdown file that is not a chat history was read as %q", harness)
	}
	empty := filepath.Join(dir, ".aider.chat.history.md")
	os.WriteFile(empty, []byte("#### typed before any run started\n"), 0o600)
	if _, _, err := LoadFile(empty); err == nil {
		t.Fatal("a history with no run in it has nothing to import")
	}
	if _, err := importAider(Input{Lines: []string{"#### hi  ", "", "Hello."}}); err == nil {
		t.Fatal("lines that do not begin a run are not one")
	}
}
