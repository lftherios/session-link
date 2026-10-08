package importers

import (
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

func writeRecent(t *testing.T, file, body string, at time.Time) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(file), 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(file, []byte(body), 0o600); err != nil {
		t.Fatal(err)
	}
	if err := os.Chtimes(file, at, at); err != nil {
		t.Fatal(err)
	}
}

func TestRecentFileSessions(t *testing.T) {
	root := t.TempDir()
	loc := Locations{Claude: filepath.Join(root, "claude"), Pi: filepath.Join(root, "pi"), Codex: filepath.Join(root, "codex")}
	now := time.Now()
	cc := filepath.Join(loc.Claude, "-work-project", "same.jsonl")
	writeRecent(t, cc, `{"type":"user","message":{"content":"Debug the failing request"}}`+"\n", now.Add(-time.Hour))
	pi := filepath.Join(loc.Pi, "--work-project--", "timestamp.jsonl")
	writeRecent(t, pi, `{"type":"session","id":"same","cwd":"/work/project"}`+"\n"+`{"type":"message","message":{"role":"user","content":[{"type":"text","text":"Compare the competitors"}]}}`+"\n", now)
	for _, idKey := range []string{"id", "session_id"} {
		writeRecent(t, filepath.Join(loc.Codex, idKey+".jsonl"), fmt.Sprintf(`{"type":"session_meta","payload":{%q:%q,"cwd":"/work/project"}}`+"\n"+`{"type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":"<environment_context>setup</environment_context>"}]}}`+"\n"+`{"type":"response_item","payload":{"role":"user","content":[{"type":"input_text","text":"Review the health endpoint"}]}}`+"\n", idKey, idKey), now.Add(-2*time.Hour))
	}
	writeRecent(t, filepath.Join(loc.Codex, "other.jsonl"), `{"type":"session_meta","payload":{"id":"other","cwd":"/work/project-other"}}`, now.Add(time.Hour))
	all, err := loc.Recent("", "/work/project/src", "", 30)
	if err != nil || len(all) != 4 {
		t.Fatalf("got %v, %v", all, err)
	}
	if all[0].Harness != "pi" || all[0].Title != "Compare the competitors" {
		t.Fatalf("newest = %+v", all[0])
	}
	if all[1].Title != "Debug the failing request" {
		t.Fatal(all[1].Title)
	}
	for _, c := range all[2:] {
		if c.Title != "Review the health endpoint" {
			t.Fatal(c.Title)
		}
	}
	// Same ID in two harnesses stays ambiguous; callers must ask for a choice.
	matches, err := loc.Recent("", "/work/project", "same", 1)
	if err != nil || len(matches) != 2 {
		t.Fatalf("matches=%v err=%v", matches, err)
	}
	// An explicit old ID is applied before the picker limit.
	old, err := loc.Recent("codex", "/work/project", "session_id", 1)
	if err != nil || len(old) != 1 || old[0].ID != "session_id" {
		t.Fatalf("old=%v err=%v", old, err)
	}
	writeRecent(t, filepath.Join(loc.Pi, "--work-project--", "new.jsonl"), `{"type":"session","id":"new"}`, now.Add(time.Hour))
	in, err := all[0].Load()
	if err != nil || !strings.Contains(in.Lines[0], `"same"`) {
		t.Fatalf("selection drifted: %+v %v", in, err)
	}
}

func TestRecentDatabaseSessions(t *testing.T) {
	for _, harness := range []string{"opencode", "hermes"} {
		t.Run(harness, func(t *testing.T) {
			file := filepath.Join(t.TempDir(), "sessions.db")
			db, err := sql.Open("sqlite", file)
			if err != nil {
				t.Fatal(err)
			}
			defer db.Close()
			exec := func(query string, args ...any) {
				t.Helper()
				if _, err := db.Exec(query, args...); err != nil {
					t.Fatal(err)
				}
			}
			loc := Locations{}
			var table, stamp string
			if harness == "opencode" {
				loc.Opencode = file
				table, stamp = "session", "time_created"
				exec(`CREATE TABLE session (id TEXT, title TEXT, directory TEXT, time_created INTEGER)`)
				exec(`CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)`)
				exec(`CREATE TABLE part (id TEXT, session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT)`)
				exec(`INSERT INTO message VALUES ('msg','older',1,'{"role":"user"}')`)
				exec(`INSERT INTO part VALUES ('part','older','msg',1,'{"type":"text","text":"Original prompt"}')`)
			} else {
				loc.Hermes = file
				table, stamp = "sessions", "started_at"
				exec(`CREATE TABLE sessions (id TEXT, title TEXT, cwd TEXT, started_at INTEGER)`)
				exec(`CREATE TABLE messages (id INTEGER, session_id TEXT, role TEXT, content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL, token_count INTEGER, finish_reason TEXT, reasoning TEXT, reasoning_content TEXT)`)
				exec(`INSERT INTO messages (id, session_id, role, content) VALUES (1,'older','user','Original prompt')`)
			}
			exec("INSERT INTO " + table + " VALUES ('older','First task','/work/project',1), ('newer','Second task','/work/project',2), ('elsewhere','Other task','/other',3)")
			candidates, err := loc.Recent(harness, "/work/project/src", "", 30)
			if err != nil || len(candidates) != 2 || candidates[0].ID != "newer" {
				t.Fatalf("%+v %v", candidates, err)
			}
			chosen, err := loc.Recent(harness, "/work/project", "older", 1)
			if err != nil || len(chosen) != 1 {
				t.Fatalf("%+v %v", chosen, err)
			}
			exec("UPDATE " + table + " SET " + stamp + "=100 WHERE id='newer'")
			in, err := chosen[0].Load()
			data, _ := json.Marshal(in.Messages)
			if err != nil || in.Session["id"] != "older" || !strings.Contains(string(data), "Original prompt") {
				t.Fatalf("pinned load: %+v %v", in, err)
			}
			// An incompatible message schema must fail, not silently omit evidence.
			if harness == "opencode" {
				exec("DROP TABLE part")
			} else {
				exec("DROP TABLE messages")
			}
			if _, err := chosen[0].Load(); err == nil {
				t.Fatal("missing message schema was silently accepted")
			}
		})
	}
}

func TestRecentKeepsReadableHarnessWhenAnotherStoreIsIncompatible(t *testing.T) {
	root := t.TempDir()
	loc := Locations{Pi: filepath.Join(root, "pi"), Hermes: filepath.Join(root, "hermes.db")}
	writeRecent(t, filepath.Join(loc.Pi, "--work-project--", "one.jsonl"), `{"type":"session","id":"one"}`, time.Now())
	db, err := sql.Open("sqlite", loc.Hermes)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := db.Exec("CREATE TABLE incompatible (id TEXT)"); err != nil {
		t.Fatal(err)
	}
	db.Close()
	candidates, err := loc.Recent("", "/work/project", "", 30)
	if len(candidates) != 1 || candidates[0].ID != "one" || err == nil || !strings.Contains(err.Error(), "hermes") {
		t.Fatalf("readable session and diagnostic must both survive: %+v %v", candidates, err)
	}
}

func recentDB(t *testing.T, file string, statements ...string) {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(file), 0o700); err != nil {
		t.Fatal(err)
	}
	db, err := sql.Open("sqlite", file)
	if err != nil {
		t.Fatal(err)
	}
	defer db.Close()
	for _, statement := range statements {
		if _, err := db.Exec(statement); err != nil {
			t.Fatalf("%s: %v", statement, err)
		}
	}
}

func candidateIDs(candidates []Candidate) string {
	ids := make([]string, len(candidates))
	for i, c := range candidates {
		ids[i] = c.ID
	}
	return strings.Join(ids, ",")
}

// A harness records a sub-agent's work as a session of its own, in the same
// directory as the session that started it. It is not what the person means
// by their session, so only an explicit id reaches it.
func TestSubAgentSessionsAreNotOfferedAsThePersonsSession(t *testing.T) {
	const cwd = "/work/project"
	now := time.Now()

	t.Run("codex", func(t *testing.T) {
		root := t.TempDir()
		t.Setenv("CODEX_HOME", root)
		loc := Locations{Codex: filepath.Join(root, "sessions")}
		rollout := func(id, source, prompt string, at time.Time) {
			writeRecent(t, filepath.Join(loc.Codex, "2026", "10", "03", "rollout-"+id+".jsonl"),
				`{"type":"session_meta","payload":{"id":"`+id+`","session_id":"main","cwd":"`+cwd+`","source":`+source+`}}`+"\n"+
					`{"type":"response_item","payload":{"type":"message","role":"user","content":[{"type":"input_text","text":"`+prompt+`"}]}}`+"\n", at)
		}
		rollout("main", `"cli"`, "Rename the handler", now.Add(-time.Hour))
		rollout("scout", `{"subagent":{"thread_spawn":{"parent_thread_id":"main","depth":1}}}`, "List the call sites", now.Add(-time.Minute))
		rollout("review", `{"subagent":"review"}`, "Review the diff", now.Add(-30*time.Second))
		rollout("memory", `{"internal":"memory_consolidation"}`, "Consolidate", now)

		listed, err := loc.Recent("codex", cwd, "", 30)
		if err != nil || candidateIDs(listed) != "main" {
			t.Fatalf("listed %q, %v", candidateIDs(listed), err)
		}
		found, ok := Latest("codex", cwd)
		if !ok {
			t.Fatal("no latest session")
		}
		if in, err := found.Load(); err != nil || !strings.Contains(in.Lines[1], "Rename the handler") {
			t.Fatalf("the latest session is not the person's own: %+v %v", in.Lines, err)
		}
		if byID, err := loc.Recent("codex", cwd, "scout", 1); err != nil || candidateIDs(byID) != "scout" {
			t.Fatalf("an explicit id must still reach a sub-agent: %q %v", candidateIDs(byID), err)
		}
	})

	t.Run("opencode", func(t *testing.T) {
		data := t.TempDir()
		t.Setenv("XDG_DATA_HOME", data)
		file := filepath.Join(data, "opencode", "opencode.db")
		recentDB(t, file,
			`CREATE TABLE session (id TEXT, parent_id TEXT, title TEXT, directory TEXT, time_created INTEGER)`,
			`CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)`,
			`CREATE TABLE part (id TEXT, session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT)`,
			`INSERT INTO session VALUES ('main', NULL, 'Rename the handler', '`+cwd+`', 1), ('task', 'main', 'List the call sites', '`+cwd+`', 2)`)

		listed, err := Locations{Opencode: file}.Recent("opencode", cwd, "", 30)
		if err != nil || candidateIDs(listed) != "main" {
			t.Fatalf("listed %q, %v", candidateIDs(listed), err)
		}
		found, ok := Latest("opencode", cwd)
		if !ok {
			t.Fatal("no latest session")
		}
		if in, err := found.Load(); err != nil || in.Session["id"] != "main" {
			t.Fatalf("the latest session is not the person's own: %+v %v", in.Session, err)
		}
		if byID, err := (Locations{Opencode: file}).Recent("opencode", cwd, "task", 1); err != nil || candidateIDs(byID) != "task" {
			t.Fatalf("an explicit id must still reach a sub-agent: %q %v", candidateIDs(byID), err)
		}
	})
}

// The schema is Hermes 0.15's, reduced to the columns discovery and import
// read. Hermes links sessions to a parent for three different reasons.
func TestHermesSessionsFollowItsOwnSessionList(t *testing.T) {
	const cwd = "/work/project"
	file := filepath.Join(t.TempDir(), "state.db")
	t.Setenv("HERMES_STATE_DB", file)
	recentDB(t, file,
		`CREATE TABLE sessions (id TEXT PRIMARY KEY, source TEXT, model_config TEXT, parent_session_id TEXT, started_at REAL, ended_at REAL, end_reason TEXT, cwd TEXT, title TEXT)`,
		`CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, role TEXT, content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL, token_count INTEGER, finish_reason TEXT, reasoning TEXT, reasoning_content TEXT, active INTEGER NOT NULL DEFAULT 1)`,
		// A conversation whose context was compressed: Hermes ends the session
		// and continues in a new one, for which it records no directory.
		`INSERT INTO sessions VALUES ('first', 'cli', NULL, NULL, 100, 200, 'compression', '`+cwd+`', 'Billing refactor')`,
		`INSERT INTO sessions VALUES ('continued', 'cli', NULL, 'first', 200, NULL, NULL, NULL, 'Billing refactor #2')`,
		// Started while its parent was live: a sub-agent or a background review.
		`INSERT INTO sessions VALUES ('delegate', 'cli', NULL, 'continued', 250, 260, 'completed', '`+cwd+`', NULL)`,
		// Two branches: one after its parent ended as branched, one marked.
		`INSERT INTO sessions VALUES ('trunk', 'cli', NULL, NULL, 300, 400, 'branched', '`+cwd+`', 'Trunk')`,
		`INSERT INTO sessions VALUES ('branch', 'cli', NULL, 'trunk', 400, NULL, NULL, '`+cwd+`', 'Branch')`,
		`INSERT INTO sessions VALUES ('marked', 'cli', '{"_branched_from":"trunk"}', 'trunk', 350, NULL, NULL, '`+cwd+`', 'Marked branch')`,
		`INSERT INTO sessions VALUES ('elsewhere', 'cli', 'not json', NULL, 500, NULL, NULL, '/other', 'Other project')`,
		// The person rewound the first exchange; Hermes keeps it as inactive.
		`INSERT INTO messages (session_id, role, content, timestamp, active) VALUES ('first', 'user', 'Delete the legacy module', 101, 0), ('first', 'assistant', 'Deleted.', 102, 0), ('first', 'user', 'Keep the legacy module and add a test', 103, 1), ('first', 'assistant', 'Test added.', 104, 1)`)
	loc := Locations{Hermes: file}

	listed, err := loc.Recent("hermes", cwd+"/src", "", 30)
	if err != nil || candidateIDs(listed) != "branch,marked,trunk,continued,first" {
		t.Fatalf("listed %q, %v", candidateIDs(listed), err)
	}
	for _, c := range listed {
		if c.Dir != cwd {
			t.Fatalf("%s listed under %q", c.ID, c.Dir)
		}
	}
	if found, ok := Latest("hermes", cwd); !ok || found.Recency != 400*int64(time.Second) {
		t.Fatalf("latest = %+v, %v", found, ok)
	}
	if byID, err := loc.Recent("hermes", cwd, "delegate", 1); err != nil || candidateIDs(byID) != "delegate" {
		t.Fatalf("an explicit id must still reach a sub-agent: %q %v", candidateIDs(byID), err)
	}
	if byID, err := loc.Recent("hermes", cwd, "continued", 1); err != nil || candidateIDs(byID) != "continued" {
		t.Fatalf("a continuation must be found in its conversation's directory: %q %v", candidateIDs(byID), err)
	}

	in, err := listed[4].Load()
	if err != nil {
		t.Fatal(err)
	}
	data, _ := json.Marshal(in.Messages)
	if len(in.Messages) != 2 || strings.Contains(string(data), "Delete the legacy module") || !strings.Contains(string(data), "Keep the legacy module") {
		t.Fatalf("rewound messages must stay out of the import: %s", data)
	}
}

func TestProjectDirsStopBeforeHomeAndRoot(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	for cwd, want := range map[string][]string{
		filepath.Join(home, "code", "proj", "src"): {filepath.Join(home, "code", "proj", "src"), filepath.Join(home, "code", "proj"), filepath.Join(home, "code")},
		home:                {home},
		"/work/project/src": {"/work/project/src", "/work/project", "/work"},
		"/":                 {"/"},
	} {
		if got := ProjectDirs(cwd); strings.Join(got, " ") != strings.Join(want, " ") {
			t.Errorf("ProjectDirs(%q) = %v, want %v", cwd, got, want)
		}
	}
}

// A session started in the home directory belongs to work done there, and
// to no project beneath it, whichever harness recorded it.
func TestHomeDirectorySessionsBelongToHomeOnly(t *testing.T) {
	home := isolateStores(t)
	now := time.Now()
	loc := DefaultLocations()
	writeRecent(t, filepath.Join(loc.Claude, claudeSlug(home), "claude.jsonl"), claudeEntry(home, "A question asked at home"), now)
	writeRecent(t, filepath.Join(piProjectDir(loc.Pi, home), "pi.jsonl"), `{"type":"session","id":"pi","cwd":"`+home+`"}`+"\n", now)
	writeRecent(t, filepath.Join(loc.Codex, "rollout-codex.jsonl"), `{"type":"session_meta","payload":{"id":"codex","cwd":"`+home+`"}}`+"\n", now)
	recentDB(t, loc.Opencode,
		`CREATE TABLE session (id TEXT, title TEXT, directory TEXT, time_created INTEGER)`,
		`INSERT INTO session VALUES ('opencode', 'At home', '`+home+`', 1)`)
	recentDB(t, loc.Hermes,
		`CREATE TABLE sessions (id TEXT, title TEXT, cwd TEXT, started_at INTEGER)`,
		`INSERT INTO sessions VALUES ('hermes', 'At home', '`+home+`', 1)`)

	atHome, err := loc.Recent("", home, "", 30)
	if err != nil || len(atHome) != 5 {
		t.Fatalf("in the home directory: %q, %v", candidateIDs(atHome), err)
	}
	project := filepath.Join(home, "code", "proj")
	if listed, err := loc.Recent("", project, "", 30); err != nil || len(listed) != 0 {
		t.Fatalf("in a project beneath it: %q, %v", candidateIDs(listed), err)
	}
	for _, harness := range []string{"claude-code", "pi", "codex", "opencode", "hermes"} {
		if _, ok := Latest(harness, home); !ok {
			t.Errorf("%s: the newest session in the home directory was not found", harness)
		}
	}
}

func TestRecentShowsThePiSessionName(t *testing.T) {
	loc := Locations{Pi: t.TempDir()}
	prompt := `{"type":"message","id":"m1","parentId":null,"message":{"role":"user","content":[{"type":"text","text":"Find the race"}]}}`
	header := `{"type":"session","id":"named","cwd":"/work/project"}`
	writeRecent(t, filepath.Join(loc.Pi, "--work-project--", "named.jsonl"),
		header+"\n"+prompt+"\n"+`{"type":"session_info","id":"i1","parentId":"m1","name":"First name"}`+"\n"+`{"type":"session_info","id":"i2","parentId":"i1","name":" Upload test race "}`+"\n", time.Now())
	writeRecent(t, filepath.Join(loc.Pi, "--work-project--", "cleared.jsonl"),
		strings.Replace(header, "named", "cleared", 1)+"\n"+prompt+"\n"+`{"type":"session_info","id":"i1","parentId":"m1","name":"Old name"}`+"\n"+`{"type":"session_info","id":"i2","parentId":"i1","name":""}`+"\n", time.Now().Add(-time.Minute))
	listed, err := loc.Recent("pi", "/work/project", "", 30)
	if err != nil || len(listed) != 2 {
		t.Fatalf("%+v %v", listed, err)
	}
	if listed[0].Name != "Upload test race" || listed[0].Title != "Upload test race" {
		t.Fatalf("named session: name=%q title=%q", listed[0].Name, listed[0].Title)
	}
	if listed[1].Name != "" || listed[1].Title != "Find the race" {
		t.Fatalf("a cleared name must fall back to the prompt: name=%q title=%q", listed[1].Name, listed[1].Title)
	}
}

// A long-running session is as recent as its last activity, like a transcript
// file is as recent as its last write.
func TestDatabaseSessionsRankByLastActivity(t *testing.T) {
	const cwd = "/work/project"
	dir := t.TempDir()
	loc := Locations{Opencode: filepath.Join(dir, "opencode.db"), Hermes: filepath.Join(dir, "state.db")}
	recentDB(t, loc.Opencode,
		`CREATE TABLE session (id TEXT, parent_id TEXT, title TEXT, directory TEXT, time_created INTEGER, time_updated INTEGER)`,
		`INSERT INTO session VALUES ('long-running', NULL, 'Started first, still going', '`+cwd+`', 1000, 9000), ('quick', NULL, 'Started later, finished', '`+cwd+`', 5000, 6000)`)
	recentDB(t, loc.Hermes,
		`CREATE TABLE sessions (id TEXT, title TEXT, cwd TEXT, started_at REAL)`,
		`CREATE TABLE messages (id INTEGER PRIMARY KEY, session_id TEXT, role TEXT, content TEXT, timestamp REAL)`,
		`INSERT INTO sessions VALUES ('long-running', 'Started first, still going', '`+cwd+`', 1), ('quick', 'Started later, finished', '`+cwd+`', 5), ('silent', 'No messages yet', '`+cwd+`', 3)`,
		`INSERT INTO messages (session_id, role, content, timestamp) VALUES ('long-running', 'user', 'still here', 9), ('quick', 'user', 'done', 6)`)
	for harness, unit := range map[string]time.Duration{"opencode": time.Millisecond, "hermes": time.Second} {
		listed, err := loc.Recent(harness, cwd, "", 30)
		if err != nil || !strings.HasPrefix(candidateIDs(listed), "long-running,quick") {
			t.Fatalf("%s listed %q, %v", harness, candidateIDs(listed), err)
		}
		first := listed[0]
		wantActive, wantStarted := 9*unit, 1*unit
		if harness == "opencode" {
			wantActive, wantStarted = 9000*unit, 1000*unit
		}
		if first.Recency != int64(wantActive) || !first.Started.Equal(time.Unix(0, int64(wantStarted))) {
			t.Fatalf("%s: active %v started %v", harness, time.Duration(first.Recency), first.Started)
		}
	}
}

// Each store keeps a sub-agent's session apart from its parent's. Loading the
// parent brings them along, and only the ones that are sub-agents.
func TestSubAgentSessionsLoadWithTheirParent(t *testing.T) {
	const cwd = "/work/project"
	names := func(agents []Agent) string {
		var ids []string
		for _, a := range agents {
			ids = append(ids, a.ID)
		}
		return strings.Join(ids, ",")
	}

	t.Run("opencode", func(t *testing.T) {
		file := filepath.Join(t.TempDir(), "opencode.db")
		recentDB(t, file,
			`CREATE TABLE session (id TEXT, parent_id TEXT, title TEXT, directory TEXT, time_created INTEGER)`,
			`CREATE TABLE message (id TEXT, session_id TEXT, time_created INTEGER, data TEXT)`,
			`CREATE TABLE part (id TEXT, session_id TEXT, message_id TEXT, time_created INTEGER, data TEXT)`,
			`INSERT INTO session VALUES ('main', NULL, 'Main', '`+cwd+`', 1), ('task', 'main', 'Task', '`+cwd+`', 2), ('deeper', 'task', 'Deeper', '`+cwd+`', 3), ('other', NULL, 'Other', '`+cwd+`', 4)`)
		in, err := loadOpencodeAt(file, "main")
		if err != nil || names(in.Agents) != "task" || names(in.Agents[0].Input.Agents) != "deeper" {
			t.Fatalf("agents %q %v", names(in.Agents), err)
		}
	})

	t.Run("hermes", func(t *testing.T) {
		file := filepath.Join(t.TempDir(), "state.db")
		recentDB(t, file,
			`CREATE TABLE sessions (id TEXT PRIMARY KEY, parent_session_id TEXT, started_at REAL, ended_at REAL, end_reason TEXT, cwd TEXT, title TEXT)`,
			`CREATE TABLE messages (id INTEGER PRIMARY KEY AUTOINCREMENT, session_id TEXT, role TEXT, content TEXT, tool_call_id TEXT, tool_calls TEXT, tool_name TEXT, timestamp REAL, token_count INTEGER, finish_reason TEXT, reasoning TEXT, reasoning_content TEXT)`,
			`INSERT INTO sessions VALUES ('main', NULL, 100, 200, 'compression', '`+cwd+`', 'Main')`,
			// Started while main was live: a delegate. Started after it ended: its continuation.
			`INSERT INTO sessions VALUES ('delegate', 'main', 150, 160, 'completed', '`+cwd+`', NULL)`,
			`INSERT INTO sessions VALUES ('continued', 'main', 200, NULL, NULL, NULL, 'Main #2')`,
			`INSERT INTO sessions VALUES ('live-delegate', 'continued', 250, NULL, NULL, '`+cwd+`', NULL)`)
		in, err := loadHermesAt(file, "main")
		if err != nil || names(in.Agents) != "delegate" {
			t.Fatalf("agents of the first session %q %v", names(in.Agents), err)
		}
		in, err = loadHermesAt(file, "continued")
		if err != nil || names(in.Agents) != "live-delegate" {
			t.Fatalf("agents of a session still running %q %v", names(in.Agents), err)
		}
	})

	t.Run("codex", func(t *testing.T) {
		root := filepath.Join(t.TempDir(), "sessions")
		t.Setenv("CODEX_HOME", filepath.Dir(root))
		rollout := func(day, id, header string) string {
			file := filepath.Join(root, "2026", "10", day, "rollout-"+id+".jsonl")
			writeRecent(t, file, `{"type":"session_meta","payload":{"id":"`+id+`","cwd":"`+cwd+`"`+header+`}}`+"\n"+
				`{"type":"response_item","payload":{"type":"message","role":"assistant","content":[{"type":"output_text","text":"`+id+` at work"}]}}`+"\n", time.Now())
			return file
		}
		spawned := func(parent, path string) string {
			return `,"parent_thread_id":"` + parent + `","agent_path":"` + path + `","source":{"subagent":{"thread_spawn":{"parent_thread_id":"` + parent + `","depth":1}}}`
		}
		main := rollout("08", "main", `,"source":"cli"`)
		// A sub-agent's rollout can land in the next day's directory.
		child := rollout("09", "child", spawned("main", "/root/checker"))
		grandchild := rollout("09", "grandchild", spawned("child", "/root/checker/prober"))
		rollout("08", "unrelated", `,"source":"cli"`)

		agents := fileAgents("codex", main)
		if len(agents) != 1 || agents[0].ID != "/root/checker" || agents[0].Name != "checker" || names(agents[0].Input.Agents) != "/root/checker/prober" {
			t.Fatalf("agents %+v", agents)
		}
		listed, err := Locations{Codex: root}.Recent("codex", cwd, "main", 1)
		if err != nil || len(listed) != 1 || strings.Join(listed[0].Related, ",") != child+","+grandchild {
			t.Fatalf("related transcripts: %+v %v", listed, err)
		}
	})
}

// A sub-agent's Codex thread opens with a copy of its parent's history. Opened
// on its own, it is that agent's work and names its own thread.
func TestCodexSubAgentThreadReadsFromWhereItsOwnHistoryStarts(t *testing.T) {
	file, err := filepath.Glob("../../../testdata/import/codex/sub-agent/rollout-*.jsonl")
	if err != nil || len(file) != 1 {
		t.Fatal(file, err)
	}
	lines, err := readLines(file[0])
	if err != nil {
		t.Fatal(err)
	}
	run, err := Registry["codex"](Input{Lines: lines, Fallback: "thread"})
	if err != nil {
		t.Fatal(err)
	}
	raw, _ := json.Marshal(run)
	metadata := run["metadata"].(map[string]any)
	if metadata["inherited_items_skipped"] == nil || strings.Contains(string(raw), "DELEGATE: have a sub-agent") {
		t.Fatalf("the parent's history was replayed as the sub-agent's: %v", metadata)
	}
	if !strings.Contains(string(raw), "NEW_TASK") || !strings.Contains(string(raw), "The shell works") {
		t.Fatal("the sub-agent's task or its answer is missing")
	}
}
