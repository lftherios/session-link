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
