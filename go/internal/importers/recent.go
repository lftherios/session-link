package importers

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"
)

// Candidate identifies a source session, not merely whichever file is newest
// when a later import happens. The loader keeps that identity through selection.
type Candidate struct {
	Found
	ID, Title, Dir, File string
	// Related are the transcripts of the session's sub-agents, which the
	// import reads along with File.
	Related []string
	// Name is a title the harness recorded and Prompt the first real prompt;
	// Title stays the terminal's fallback of either, or the ID.
	Name, Prompt string
	Started      time.Time
}

// Locations makes discovery testable without reading or changing the user's
// real harness stores. Defaults follow the same conventions as import.
type Locations struct {
	Claude, Pi, Omp, Codex, Opencode, Hermes, Dsh string
}

func DefaultLocations() Locations {
	return Locations{
		Claude: claudeProjectsDir(),
		Pi:     piSessionsDir(),
		Omp:    ompSessionsDir(),
		Codex:  codexSessionsDir(), Opencode: opencodeDBPath(), Hermes: hermesDBPath(),
		Dsh: dshSessionsDir(),
	}
}

// ProjectDirs lists the directories whose sessions belong to work in cwd: cwd
// itself, then each parent, because an agent is usually started at a project
// root and used from inside it. The walk stops before the home directory and
// before the filesystem root; a session started in either is not this
// project's. Both still count when cwd is that directory itself.
func ProjectDirs(cwd string) []string {
	dir := filepath.Clean(cwd)
	dirs := []string{dir}
	for dir != home() {
		parent := filepath.Dir(dir)
		if parent == home() || parent == filepath.Dir(parent) {
			break
		}
		dirs = append(dirs, parent)
		dir = parent
	}
	return dirs
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
	dirs := ProjectDirs(cwd)
	project := func(dir string) bool {
		dir = filepath.Clean(dir)
		for _, own := range dirs {
			if dir == own {
				return true
			}
		}
		return false
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
		if h == "pi" || h == "omp" {
			sid = strOr(piHeader(file)["id"], sid)
		}
		if h == "dsh" {
			sid = strOr(head["id"], filepath.Base(filepath.Dir(file)))
		}
		if id != "" && sid != id {
			return
		}
		candidates = append(candidates, Candidate{Found: fileInput(file, h, info.ModTime().UnixNano()), ID: sid, Dir: dir, File: file})
	}
	for _, h := range []string{"claude-code", "pi", "omp"} {
		if harness != "" && harness != h {
			continue
		}
		if (h == "pi" && loc.Pi == "") || (h == "omp" && loc.Omp == "") || (h == "claude-code" && loc.Claude == "") {
			continue
		}
		for _, dir := range dirs {
			var files []string
			if h == "pi" {
				files = piTranscripts(loc.Pi, dir)
			} else if h == "omp" {
				files = ompTranscripts(loc.Omp, dir)
			} else {
				files = claudeTranscripts(loc.Claude, dir)
			}
			for _, file := range files {
				addFile(file, h, dir)
			}
		}
	}
	if loc.Codex != "" && (harness == "" || harness == "codex") {
		filepath.WalkDir(loc.Codex, func(file string, entry os.DirEntry, err error) error {
			if err != nil || entry.IsDir() || !strings.HasSuffix(file, ".jsonl") {
				return nil
			}
			head := firstJSONLine(file)
			meta := m(head["payload"])
			dir := strOr(meta["cwd"], "")
			// Only an explicit id reaches a sub-agent's own rollout.
			if head["type"] == "session_meta" && project(dir) && (id != "" || !codexAuxiliaryThread(meta)) {
				addFile(file, "codex", dir)
			}
			return nil
		})
	}
	if loc.Dsh != "" && (harness == "" || harness == "dsh") {
		for _, dir := range dirs {
			// Only an explicit id reaches a sub-agent's own session.
			for _, file := range dshSessions(loc.Dsh, dir, id != "") {
				addFile(file, "dsh", dir)
			}
		}
	}
	if harness == "" || harness == "aider" {
		for _, dir := range dirs {
			file := filepath.Join(dir, aiderChatName)
			info, err := os.Stat(file)
			if err != nil || !info.Mode().IsRegular() {
				continue
			}
			runs, _ := aiderRuns(file)
			for i, run := range runs {
				if id != "" && run.id != id {
					continue
				}
				// A run was last active when the file was last written, or
				// before the run after it began.
				active := info.ModTime().UnixNano()
				if i+1 < len(runs) {
					active = runs[i+1].started.UnixNano()
				}
				sid := run.id
				candidates = append(candidates, Candidate{
					Found: Found{Harness: "aider", Recency: active, load: func() (Input, error) { return aiderInput(file, sid) }},
					ID:    sid, Dir: dir, File: file, Prompt: aiderPrompt(run), Started: run.started,
				})
			}
		}
	}
	for _, h := range []string{"opencode", "hermes"} {
		if harness != "" && harness != h {
			continue
		}
		dbPath, sessions, start, scale := loc.Opencode, opencodeSessions, "time_created", int64(1e6)
		if h == "hermes" {
			dbPath, sessions, start, scale = loc.Hermes, hermesSessions, "started_at", 1e9
		}
		db, err := openDB(dbPath)
		if err != nil {
			continue
		} // A harness that isn't installed is normal.
		for _, dir := range dirs {
			rows, err := sessions(db, dir, id, limit)
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
				candidate := Candidate{Found: Found{Harness: h, Recency: int64(numOr(row["active_at"], 0)) * scale, load: loader}, ID: sid, Title: strOr(row["title"], ""), Dir: dir}
				if started := int64(numOr(row[start], 0)); started > 0 {
					candidate.Started = time.Unix(0, started*scale)
				}
				candidates = append(candidates, candidate)
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
	var threads map[string][]codexThread
	for i := range candidates {
		c := &candidates[i]
		if c.Harness == "aider" {
			// Aider gives a run no title; what the listing has is its first prompt.
			c.Title = c.Prompt
		} else if c.Harness == "dsh" {
			c.Related = agentFiles(c.Harness, c.File, c.ID, nil)
			c.Name, c.Prompt, c.Started = dshTitles(c.File)
			c.Title = c.Name
			if c.Title == "" {
				c.Title = c.Prompt
			}
		} else if c.File != "" {
			if c.Harness == "codex" && threads == nil {
				threads = codexThreads(loc.Codex)
			}
			c.Related = agentFiles(c.Harness, c.File, c.ID, threads)
			_, c.Name, c.Prompt, c.Started = peekTitles(c.File, 40)
			if latest := latestAITitle(c.File); latest != "" {
				c.Name = latest
			}
			if c.Harness == "pi" {
				c.Name = piSessionName(c.File)
			}
			if c.Harness == "omp" {
				c.Name = ompSessionTitle(c.File)
			}
			c.Title = c.Name
			if c.Title == "" {
				c.Title = c.Prompt
			}
		} else {
			c.Name = c.Title // database-backed harnesses store their own titles
		}
		if c.Title == "" {
			c.Title = c.ID
		}
	}
	return candidates, errors.Join(problems...)
}
