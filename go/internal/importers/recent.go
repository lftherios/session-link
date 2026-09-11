package importers

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
)

// Candidate identifies a source session, not merely whichever file is newest
// when a later import happens. The loader keeps that identity through selection.
type Candidate struct {
	Found
	ID, Title, Dir, File string
}

// Locations makes discovery testable without reading or changing the user's
// real harness stores. Defaults follow the same conventions as import.
type Locations struct {
	Claude, Pi, Codex, Opencode, Hermes string
}

func DefaultLocations() Locations {
	return Locations{
		Claude: filepath.Join(home(), ".claude", "projects"),
		Pi:     filepath.Join(home(), ".pi", "agent", "sessions"),
		Codex:  codexSessionsDir(), Opencode: opencodeDBPath(), Hermes: hermesDBPath(),
	}
}

// Recent includes multiple sessions in the same harness. A populated parent
// project is included when invoked from a subdirectory. ID filters before the
// display limit, so an older explicitly named session is still reachable.
func (loc Locations) Recent(harness, cwd, id string, limit int) ([]Candidate, error) {
	if harness != "" && Registry[harness] == nil {
		return nil, fmt.Errorf("unknown agent %q", harness)
	}
	if limit <= 0 {
		limit = 30
	}
	var candidates []Candidate
	var problems []error
	project := func(dir string) bool {
		dir = filepath.Clean(dir)
		return dir != "." && (dir == filepath.Clean(cwd) || strings.HasPrefix(filepath.Clean(cwd), dir+string(os.PathSeparator)))
	}
	addFile := func(file, h, dir string) {
		info, err := os.Stat(file)
		if err != nil || !info.Mode().IsRegular() {
			return
		}
		head := firstJSONLine(file)
		sid := strings.TrimSuffix(filepath.Base(file), ".jsonl")
		if h == "codex" {
			payload := m(head["payload"])
			sid = strOr(payload["id"], strOr(payload["session_id"], sid))
		}
		if h == "pi" {
			sid = strOr(head["id"], sid)
		}
		if id != "" && sid != id {
			return
		}
		candidates = append(candidates, Candidate{Found: fileInput(file, h, info.ModTime().UnixNano()), ID: sid, Dir: dir, File: file})
	}
	for _, h := range []string{"claude-code", "pi"} {
		if harness != "" && harness != h {
			continue
		}
		if (h == "pi" && loc.Pi == "") || (h == "claude-code" && loc.Claude == "") {
			continue
		}
		for dir := filepath.Clean(cwd); dir != filepath.Dir(dir) && dir != home(); dir = filepath.Dir(dir) {
			encoded := strings.ReplaceAll(strings.ReplaceAll(dir, "/", "-"), ".", "-")
			base := filepath.Join(loc.Claude, encoded)
			if h == "pi" {
				base = filepath.Join(loc.Pi, "--"+strings.ReplaceAll(strings.TrimPrefix(dir, "/"), "/", "-")+"--")
			}
			entries, _ := os.ReadDir(base)
			for _, entry := range entries {
				if !entry.IsDir() && strings.HasSuffix(entry.Name(), ".jsonl") {
					addFile(filepath.Join(base, entry.Name()), h, dir)
				}
			}
		}
	}
	if loc.Codex != "" && (harness == "" || harness == "codex") {
		filepath.WalkDir(loc.Codex, func(file string, entry os.DirEntry, err error) error {
			if err != nil || entry.IsDir() || !strings.HasSuffix(file, ".jsonl") {
				return nil
			}
			head := firstJSONLine(file)
			dir := strOr(m(head["payload"])["cwd"], "")
			if head["type"] == "session_meta" && project(dir) {
				addFile(file, "codex", dir)
			}
			return nil
		})
	}
	for _, h := range []string{"opencode", "hermes"} {
		if harness != "" && harness != h {
			continue
		}
		dbPath, table, directory, stamp, scale := loc.Opencode, "session", "directory", "time_created", int64(1e6)
		if h == "hermes" {
			dbPath, table, directory, stamp, scale = loc.Hermes, "sessions", "cwd", "started_at", 1e9
		}
		db, err := openDB(dbPath)
		if err != nil {
			continue
		} // A harness that isn't installed is normal.
		for dir := filepath.Clean(cwd); dir != filepath.Dir(dir) && dir != home(); dir = filepath.Dir(dir) {
			query := "SELECT id, title, " + directory + ", " + stamp + " FROM " + table + " WHERE " + directory + " = ?"
			args := []any{dir}
			if id != "" {
				query += " AND id = ?"
				args = append(args, id)
			}
			query += " ORDER BY " + stamp + " DESC, id LIMIT ?"
			args = append(args, limit)
			rows, err := queryRows(db, query, args...)
			if err != nil {
				problems = append(problems, fmt.Errorf("cannot read %s sessions: %w", h, err))
				break
			}
			for _, row := range rows {
				sid := strOr(row["id"], "")
				path, selectedHarness := dbPath, h
				loader := func() (Input, error) {
					if selectedHarness == "opencode" {
						return loadOpencodeAt(path, sid)
					}
					return loadHermesAt(path, sid)
				}
				candidates = append(candidates, Candidate{Found: Found{Harness: h, Recency: int64(numOr(row[stamp], 0)) * scale, load: loader}, ID: sid, Title: strOr(row["title"], ""), Dir: dir})
			}
		}
		db.Close()
	}
	sort.Slice(candidates, func(i, j int) bool {
		if candidates[i].Recency != candidates[j].Recency {
			return candidates[i].Recency > candidates[j].Recency
		}
		return candidates[i].Harness+":"+candidates[i].ID < candidates[j].Harness+":"+candidates[j].ID
	})
	if id == "" && len(candidates) > limit {
		candidates = candidates[:limit]
	}
	for i := range candidates {
		c := &candidates[i]
		if c.File != "" {
			_, c.Title = peekTranscript(c.File, 40)
		}
		if c.Title == "" {
			c.Title = c.ID
		}
	}
	return candidates, errors.Join(problems...)
}
