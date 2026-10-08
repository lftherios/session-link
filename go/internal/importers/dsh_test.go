package importers

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// dshLogText is a small dsh v4 log: a header and the events of one turn.
func dshLogText(id, cwd string, header string, events ...string) string {
	lines := []string{`{"type":"session","version":4,"id":"` + id + `","createdAt":1791451208304,"cwd":"` + cwd + `","isSeeded":false,"delegationDepth":0` + header + `}`}
	return strings.Join(append(lines, events...), "\n") + "\n"
}

const (
	dshTyped     = `{"type":"user/message","seq":8,"time":1791451208534,"data":{"content":[{"type":"text","text":"Find the race in the upload test"}],"source":{"kind":"user"},"role":"user","id":"m1"},"surfaceOp":"append"}`
	dshFallback  = `{"type":"session/title","seq":12,"time":1791451208539,"data":{"title":"Find the race in the","messageSeqs":[8],"source":{"kind":"fallback"}}}`
	dshTitled    = `{"type":"session/title","seq":18,"time":1791451208588,"data":{"title":"Upload test race","messageSeqs":[8],"source":{"kind":"provider","provider":"session-title-first-prompt-llm"}}}`
	dshReply     = `{"type":"assistant/message","seq":15,"time":1791451208583,"data":{"turn":1,"step":1,"message":{"role":"assistant","content":[{"type":"text","text":"Looking."}],"source":{"kind":"model","provider":"deepseek-official","model":"deepseek-flash"},"id":"m2"}},"surfaceOp":"append"}`
	dshStepStart = `{"type":"step/start","seq":6,"time":1791451208531,"data":{"turn":1,"step":1}}`
)

func TestDshStoreFollowsDshHome(t *testing.T) {
	home := isolateStores(t)
	if got := dshSessionsDir(); got != filepath.Join(home, ".dsh", "sessions") {
		t.Fatalf("default store: %s", got)
	}
	// dsh treats a blank DSH_HOME as unset, and expands a leading ~.
	t.Setenv("DSH_HOME", "   ")
	if got := dshSessionsDir(); got != filepath.Join(home, ".dsh", "sessions") {
		t.Fatalf("blank DSH_HOME: %s", got)
	}
	t.Setenv("DSH_HOME", "~/work-dsh")
	if got := dshSessionsDir(); got != filepath.Join(home, "work-dsh", "sessions") {
		t.Fatalf("DSH_HOME under home: %s", got)
	}
	other := t.TempDir()
	t.Setenv("DSH_HOME", other)
	if got := dshSessionsDir(); got != filepath.Join(other, "sessions") {
		t.Fatalf("DSH_HOME: %s", got)
	}
}

func TestDshNamesDirectoriesAsDshDoes(t *testing.T) {
	for cwd, want := range map[string]string{
		"/work/checks":                 "--work-checks--",
		"/work/a/-b":                   "--work-a--b--", // a dash after a separator stays
		"/work//double":                "--work-double--",
		"/work/my app/ü":               "--work-my~0020app-~00FC--",
		`C:\Users\dev\proj`:            "--C-Users-dev-proj--",
		"/":                            "--root--",
		"/work/tilde~1":                "--work-tilde~007E1--",
		"/" + strings.Repeat("a", 300): "--" + strings.Repeat("a", 251) + "--",
	} {
		if got := dshProjectKey(cwd); got != want {
			t.Errorf("project directory for %q: %s, want %s", cwd, got, want)
		}
	}
	for id, want := range map[string]string{
		"session-449e7620-3a4f": "session-449e7620-3a4f",
		"a/b":                   "a~002Fb",
		"..":                    "~002E~002E",
		".":                     "~002E",
		"x~y z":                 "x~007Ey~0020z",
		"😀":                     "~D83D~DE00", // by UTF-16 code unit, as dsh does
	} {
		if got := dshSegment(id); got != want {
			t.Errorf("session directory for %q: %s, want %s", id, got, want)
		}
	}
}

func TestDshSessionsAreFoundWhereDshKeepsThem(t *testing.T) {
	home := isolateStores(t)
	root := filepath.Join(home, ".dsh", "sessions")
	project := "/work/upload"
	now := time.Now()
	dir := filepath.Join(root, dshProjectKey(project))
	writeRecent(t, filepath.Join(dir, "session-top", "session.v4.jsonl"), dshLogText("session-top", project, "", dshTyped, dshFallback, dshStepStart, dshReply, dshTitled), now)
	// dsh keeps a generation it has moved on from; the highest is the session.
	writeRecent(t, filepath.Join(dir, "session-top", "session.v3.jsonl"), `{"type":"session","version":3,"id":"session-top","createdAt":1,"cwd":"/elsewhere","isSeeded":false,"delegationDepth":0}`+"\n", now.Add(time.Hour))
	writeRecent(t, filepath.Join(dir, "child-1", "session.v4.jsonl"), dshLogText("child-1", project, `,"parentSession":"session-top","origin":"subagent"`, dshTyped), now.Add(time.Minute))
	writeRecent(t, filepath.Join(dir, "session-untitled", "session.v4.jsonl"), dshLogText("session-untitled", project, "", dshTyped, dshFallback), now.Add(-time.Hour))
	// "/work:upload" is filed in the same directory; its header says it is another project's.
	writeRecent(t, filepath.Join(dir, "session-other", "session.v4.jsonl"), dshLogText("session-other", "/work:upload", "", dshTyped), now)
	writeRecent(t, filepath.Join(root, dshProjectKey("/work/elsewhere"), "session-far", "session.v4.jsonl"), dshLogText("session-far", "/work/elsewhere", "", dshTyped), now)

	loc := Locations{Dsh: root}
	listed, err := loc.Recent("dsh", project, "", 30)
	if err != nil {
		t.Fatal(err)
	}
	if len(listed) != 2 || listed[0].ID != "session-top" || listed[1].ID != "session-untitled" {
		t.Fatalf("sessions offered for the project: %+v", listed)
	}
	if listed[0].Name != "Upload test race" || listed[0].Title != "Upload test race" || listed[0].Prompt != "Find the race in the upload test" {
		t.Fatalf("title and prompt: %q %q %q", listed[0].Name, listed[0].Title, listed[0].Prompt)
	}
	// The title dsh cuts from the prompt is not a title; the prompt stands in.
	if listed[1].Name != "" || listed[1].Title != "Find the race in the upload test" {
		t.Fatalf("a session dsh has not titled: %q %q", listed[1].Name, listed[1].Title)
	}
	if !listed[0].Started.Equal(time.UnixMilli(1791451208304)) {
		t.Fatalf("started: %v", listed[0].Started)
	}
	in, err := listed[0].Load()
	if err != nil || len(in.Lines) != 6 || !strings.Contains(in.Lines[0], `"version":4`) {
		t.Fatalf("the highest generation is the one read: %v %d lines", err, len(in.Lines))
	}
	// Only its id reaches a sub-agent's own session.
	child, err := loc.Recent("dsh", project, "child-1", 30)
	if err != nil || len(child) != 1 || child[0].ID != "child-1" {
		t.Fatalf("sub-agent by id: %+v %v", child, err)
	}
	if found, ok := Latest("dsh", project); !ok || found.Harness != "dsh" {
		t.Fatal("the newest session of the project is not found through DSH_HOME's default")
	}
	if file, ok := DshTranscript(project, "session-untitled"); !ok || filepath.Base(filepath.Dir(file)) != "session-untitled" {
		t.Fatalf("log by session id: %s %v", file, ok)
	}
	if elsewhere, ok := NewestAnywhere("dsh"); !ok || elsewhere.Dir == "" || elsewhere.Title == "" {
		t.Fatalf("newest anywhere: %+v", elsewhere)
	}
}

func TestACommandRunByDshIsAboutTheSessionRunningIt(t *testing.T) {
	home := isolateStores(t)
	root := filepath.Join(home, ".dsh", "sessions")
	now := time.Now()
	running := filepath.Join(root, dshProjectKey("/work/upload"), "session-running", "session.v4.jsonl")
	writeRecent(t, running, dshLogText("session-running", "/work/upload", "", dshTyped), now.Add(-time.Hour))
	writeRecent(t, filepath.Join(root, dshProjectKey("/work/upload"), "session-newer", "session.v4.jsonl"), dshLogText("session-newer", "/work/upload", "", dshTyped), now)
	id := func(found *Found) string {
		in, err := found.Load()
		if err != nil {
			t.Fatal(err)
		}
		var header map[string]any
		json.Unmarshal([]byte(in.Lines[0]), &header)
		return strOr(header["id"], "")
	}
	if found, ok := Latest("dsh", "/work/upload"); !ok || id(found) != "session-newer" {
		t.Fatal("outside dsh, the newest session of the project is the one")
	}
	// dsh sets these for every command of its shell tool.
	t.Setenv("DSH_SHELL", "1")
	t.Setenv("DSH_SESSION_ID", "session-running")
	for _, harness := range []string{"dsh", ""} {
		// The tool may be working in another directory than the session started in.
		if found, ok := Latest(harness, "/work/upload/deeper"); !ok || found.Harness != "dsh" || id(found) != "session-running" {
			t.Fatalf("from dsh's shell tool, --from %q must find the session running it", harness)
		}
	}
	if _, ok := Latest("pi", "/work/upload"); ok {
		t.Fatal("another agent asked for by name is not answered with dsh's session")
	}
	t.Setenv("DSH_SESSION_ID", "session-gone")
	if found, ok := Latest("dsh", "/work/upload"); !ok || id(found) != "session-newer" {
		t.Fatal("an id that names no session falls back to the newest")
	}
}

func TestSniffTellsDshFromPi(t *testing.T) {
	if got := sniff([]string{`{"type":"session","version":4,"id":"s","createdAt":1,"isSeeded":false,"delegationDepth":0}`}); got != "dsh" {
		t.Fatalf("a dsh log was read as %q", got)
	}
	if got := sniff([]string{`{"type":"session","version":3,"id":"s","timestamp":"2026-10-07T10:00:00.000Z","cwd":"/w"}`}); got != "pi" {
		t.Fatalf("a pi transcript was read as %q", got)
	}
}

func TestDshReadsACompressedLogCutOffMidWrite(t *testing.T) {
	logs, _ := filepath.Glob(filepath.Join("..", "..", "..", "testdata", "import", "dsh", "v4-run", "sessions", "*", "*", "session.v4.jsonl.zstd"))
	if len(logs) != 1 {
		t.Fatalf("the compressed fixture: %v", logs)
	}
	whole, err := readLines(logs[0])
	if err != nil || len(whole) < 20 {
		t.Fatalf("the fixture's frames: %d lines, %v", len(whole), err)
	}
	raw, _ := os.ReadFile(logs[0])
	cut := filepath.Join(t.TempDir(), "session.v4.jsonl.zstd")
	if err := os.WriteFile(cut, raw[:len(raw)-40], 0o600); err != nil {
		t.Fatal(err)
	}
	kept, err := readLines(cut)
	if err != nil {
		t.Fatalf("a log whose last frame is cut off must still be read: %v", err)
	}
	if len(kept) == 0 || len(kept) >= len(whole) || strings.Join(kept, "\n") != strings.Join(whole[:len(kept)], "\n") {
		t.Fatalf("what was complete before the cut is kept as it was: %d of %d lines", len(kept), len(whole))
	}
	if header := firstJSONLine(cut); strOr(header["type"], "") != "session" {
		t.Fatalf("header of a compressed log: %+v", header)
	}
}

func TestDshRefusesAFormatItDoesNotKnow(t *testing.T) {
	for _, version := range []string{"2", "5"} {
		_, err := importDsh(Input{Lines: []string{`{"type":"session","version":` + version + `,"id":"s","createdAt":1,"isSeeded":false,"delegationDepth":0}`}})
		if err == nil || !strings.Contains(err.Error(), "v"+version) {
			t.Fatalf("format v%s: %v", version, err)
		}
	}
	if _, err := importDsh(Input{Lines: []string{dshTyped}}); err == nil {
		t.Fatal("a log without a header is not a session")
	}
}

// v3, which the dsh 0.1 releases wrote, keeps a tool's result inside one
// block of a user-role message. This is that shape as dsh's own migration to
// v4 describes it, not a log from a run.
func TestDshReadsAV3ToolResult(t *testing.T) {
	run, err := importDsh(Input{Lines: []string{
		`{"type":"session","version":3,"id":"s","createdAt":1791451208304,"cwd":"/work/upload","isSeeded":false,"delegationDepth":0}`,
		dshTyped, dshStepStart, dshReply,
		`{"type":"tool/call","seq":16,"time":1791451208584,"data":{"turn":1,"step":1,"callId":"call-1","name":"bash","arguments":"{\"command\":\"ls\"}"}}`,
		`{"type":"tool/result","seq":19,"time":1791451208601,"data":{"turn":1,"step":1,"message":{"role":"user","source":{"kind":"tool","callId":"call-1"},"content":[{"type":"tool-result","toolCallId":"call-1","content":[{"type":"text","text":"ls: no such directory"}],"isError":true}],"id":"m3"}},"sourceEventSeqs":[16],"surfaceOp":"append"}`,
	}})
	if err != nil {
		t.Fatal(err)
	}
	tool := m(arr(run["spans"])[2])
	if tool["type"] != "tool_call" || tool["status"] != "error" || m(tool["output"])["result"] != "ls: no such directory" || m(tool["output"])["is_error"] != true {
		t.Fatalf("v3 tool result: %+v", tool)
	}
	if args := m(m(tool["input"])["arguments"]); args["command"] != "ls" {
		t.Fatalf("arguments are read from the JSON text dsh keeps: %+v", tool["input"])
	}
	if m(run["metadata"])["session_format"] != 3 {
		t.Fatalf("metadata: %+v", run["metadata"])
	}
}

func TestDshKeepsWhatItCannotShowAndWhoWroteWhat(t *testing.T) {
	run, err := importDsh(Input{Lines: []string{
		`{"type":"session","version":4,"id":"s","createdAt":1791451208304,"isSeeded":false,"delegationDepth":0}`,
		`{"type":"user/message","seq":1,"time":1791451208534,"data":{"content":[{"type":"text","text":"What is in the picture?"},{"type":"image","attachment":{"attachmentId":"sha256:` + strings.Repeat("ab", 32) + `","mediaType":"image/png","bytes":73}},{"type":"file","attachment":{"attachmentId":"sha256:` + strings.Repeat("cd", 32) + `","name":"notes.txt"}}],"source":{"kind":"user"},"role":"user","id":"m1"},"surfaceOp":"append"}`,
		`{"type":"user/message","seq":2,"time":1791451208535,"data":{"content":[{"type":"text","text":"Earlier turns, condensed."}],"source":{"kind":"compact-checkpoint","compactionId":"c1"},"role":"user","id":"m2"},"surfaceOp":"append"}`,
		dshStepStart,
		`{"type":"assistant/message","seq":15,"time":1791451208583,"data":{"turn":1,"step":1,"interrupted":true,"message":{"role":"assistant","content":[{"type":"text","text":"It sh"}],"source":{"kind":"model"},"id":"m3"}},"surfaceOp":"append"}`,
	}})
	if err != nil {
		t.Fatal(err)
	}
	call := m(arr(run["spans"])[1])
	messages := arr(m(call["input"])["messages"])
	typed, context := m(messages[0]), m(messages[1])
	if typed["role"] != "user" || context["role"] != "system" {
		t.Fatalf("only what the person wrote is theirs: %v %v", typed["role"], context["role"])
	}
	parts := arr(typed["content"])
	// The image's bytes are not at hand and a file is never inlined: both stay as recorded.
	if m(parts[1])["type"] != "data" || m(parts[2])["type"] != "data" || m(m(m(parts[1])["data"])["attachment"])["bytes"] != float64(73) {
		t.Fatalf("an image without its bytes and a file: %+v", parts[1:])
	}
	if m(call["metadata"])["interrupted"] != true || m(call["model"])["id"] != "unknown" {
		t.Fatalf("an interrupted reply and an unnamed model: %+v %+v", call["metadata"], call["model"])
	}
	if run["name"] != "What is in the picture?" {
		t.Fatalf("name: %v", run["name"])
	}
}
