package importers

import (
	"bufio"
	"database/sql"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"

	_ "modernc.org/sqlite" // pure-Go SQLite driver — no cgo
)

// Found is a discovered session ready to load into Input.
type Found struct {
	Harness string
	Recency int64 // ns since epoch; auto-detect picks the max across harnesses
	load    func() (Input, error)
}

func (f Found) Load() (Input, error) { return f.load() }

/* ---------------------------------------------------- path conventions */

func home() string {
	h, _ := os.UserHomeDir()
	return h
}

// ClaudeProjectDir is exported so the CLI can resolve a pasted session id
// (Claude Code names transcripts <uuid>.jsonl inside it).
func ClaudeProjectDir(cwd string) string {
	enc := strings.ReplaceAll(strings.ReplaceAll(cwd, "/", "-"), ".", "-")
	return filepath.Join(home(), ".claude", "projects", enc)
}

func piProjectDir(cwd string) string {
	enc := strings.ReplaceAll(strings.TrimPrefix(cwd, "/"), "/", "-")
	return filepath.Join(home(), ".pi", "agent", "sessions", "--"+enc+"--")
}

func codexSessionsDir() string {
	if h := os.Getenv("CODEX_HOME"); h != "" {
		return filepath.Join(h, "sessions")
	}
	return filepath.Join(home(), ".codex", "sessions")
}

func opencodeDBPath() string {
	dataHome := os.Getenv("XDG_DATA_HOME")
	if dataHome == "" {
		dataHome = filepath.Join(home(), ".local", "share")
	}
	return filepath.Join(dataHome, "opencode", "opencode.db")
}

func hermesDBPath() string {
	if p := os.Getenv("HERMES_STATE_DB"); p != "" {
		return p
	}
	return filepath.Join(home(), ".hermes", "state.db")
}

/* -------------------------------------------------------- file loaders */

func readLines(file string) ([]string, error) {
	raw, err := os.ReadFile(file)
	if err != nil {
		return nil, err
	}
	var lines []string
	for _, l := range strings.Split(string(raw), "\n") {
		if strings.TrimSpace(l) != "" {
			lines = append(lines, l)
		}
	}
	return lines, nil
}

func fileInput(file, harness string, recency int64) Found {
	return Found{Harness: harness, Recency: recency, load: func() (Input, error) {
		lines, err := readLines(file)
		if err != nil {
			return Input{}, err
		}
		base := filepath.Base(file)
		return Input{Lines: lines, Fallback: strings.TrimSuffix(base, ".jsonl")}, nil
	}}
}

// newestFile returns the most-recently-modified .jsonl under dir and its mtime.
func newestFile(dir string) (string, int64) {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return "", 0
	}
	best, bestT := "", int64(-1)
	for _, e := range entries {
		if e.IsDir() || !strings.HasSuffix(e.Name(), ".jsonl") {
			continue
		}
		info, err := e.Info()
		if err != nil {
			continue
		}
		if t := info.ModTime().UnixNano(); t > bestT {
			bestT, best = t, filepath.Join(dir, e.Name())
		}
	}
	return best, bestT
}

/* ------------------------------------------------------------- sqlite */

func openDB(path string) (*sql.DB, error) {
	if _, err := os.Stat(path); err != nil {
		return nil, err
	}
	return sql.Open("sqlite", "file:"+path+"?mode=ro")
}

// queryRows runs q and returns each row as a column→value map.
func queryRows(db *sql.DB, q string, args ...any) ([]map[string]any, error) {
	rows, err := db.Query(q, args...)
	if err != nil {
		return nil, err
	}
	defer rows.Close()
	cols, _ := rows.Columns()
	var out []map[string]any
	for rows.Next() {
		vals := make([]any, len(cols))
		ptrs := make([]any, len(cols))
		for i := range vals {
			ptrs[i] = &vals[i]
		}
		if err := rows.Scan(ptrs...); err != nil {
			return nil, err
		}
		row := map[string]any{}
		for i, c := range cols {
			row[c] = normalizeSQL(vals[i])
		}
		out = append(out, row)
	}
	return out, rows.Err()
}

// normalizeSQL coerces driver values to the JSON-native shapes the mappers
// expect (float64 numbers, string text) so a Go row matches a JS row.
func normalizeSQL(v any) any {
	switch x := v.(type) {
	case int64:
		return float64(x)
	case []byte:
		return string(x)
	default:
		return x
	}
}

/* --------------------------------------------------- session discovery */

// Latest finds the newest session for the given harness + cwd, or "" harness
// if none. harness "" means auto-detect across all.
func Latest(harness, cwd string) (*Found, bool) {
	try := map[string]func(string) (*Found, bool){
		"claude-code": latestClaude,
		"pi":          latestPi,
		"codex":       latestCodex,
		"opencode":    latestOpencode,
		"hermes":      latestHermes,
	}
	if harness != "" {
		if fn := try[harness]; fn != nil {
			return fn(cwd)
		}
		return nil, false
	}
	// Auto-detect: the newest session ACROSS every harness by recency, like
	// the JS importer (not first-found in a fixed order).
	var best *Found
	for _, h := range []string{"claude-code", "pi", "codex", "opencode", "hermes"} {
		if f, ok := try[h](cwd); ok {
			if best == nil || f.Recency > best.Recency {
				best = f
			}
		}
	}
	return best, best != nil
}

func latestClaude(cwd string) (*Found, bool) {
	if f, t := newestFile(ClaudeProjectDir(cwd)); f != "" {
		found := fileInput(f, "claude-code", t)
		return &found, true
	}
	return nil, false
}

func latestPi(cwd string) (*Found, bool) {
	if f, t := newestFile(piProjectDir(cwd)); f != "" {
		found := fileInput(f, "pi", t)
		return &found, true
	}
	return nil, false
}

func latestCodex(cwd string) (*Found, bool) {
	dir := codexSessionsDir()
	type roll struct {
		path string
		mod  int64
	}
	var rolls []roll
	filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() || !strings.HasSuffix(p, ".jsonl") {
			return nil
		}
		if info, err := d.Info(); err == nil {
			rolls = append(rolls, roll{p, info.ModTime().UnixNano()})
		}
		return nil
	})
	sort.Slice(rolls, func(i, j int) bool { return rolls[i].mod > rolls[j].mod })
	if len(rolls) > 500 {
		rolls = rolls[:500]
	}
	for _, r := range rolls {
		head := firstJSONLine(r.path)
		if strOr(head["type"], "") == "session_meta" {
			if strOr(m(head["payload"])["cwd"], "") == cwd {
				found := fileInput(r.path, "codex", r.mod)
				return &found, true
			}
		}
	}
	return nil, false
}

func firstJSONLine(file string) map[string]any {
	f, err := os.Open(file)
	if err != nil {
		return nil
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 0, 64*1024), 16*1024*1024) // session_meta embeds the whole system prompt
	if sc.Scan() {
		var v map[string]any
		if json.Unmarshal(sc.Bytes(), &v) == nil {
			return v
		}
	}
	return nil
}

func latestOpencode(cwd string) (*Found, bool) {
	db, err := openDB(opencodeDBPath())
	if err != nil {
		return nil, false
	}
	rows, err := queryRows(db, "SELECT id, time_created FROM session WHERE directory = ? ORDER BY time_created DESC LIMIT 1", cwd)
	db.Close()
	if err != nil || len(rows) == 0 {
		return nil, false
	}
	id, _ := rows[0]["id"].(string)
	// opencode time_created is ms; scale to ns to compare with file mtimes.
	found := Found{Harness: "opencode", Recency: int64(numOr(rows[0]["time_created"], 0)) * 1e6, load: func() (Input, error) { return loadOpencode(id) }}
	return &found, true
}

func loadOpencode(id string) (Input, error) {
	db, err := openDB(opencodeDBPath())
	if err != nil {
		return Input{}, err
	}
	defer db.Close()
	sessions, err := queryRows(db, "SELECT * FROM session WHERE id = ?", id)
	if err != nil {
		return Input{}, err
	}
	if len(sessions) == 0 {
		return Input{}, fmt.Errorf("opencode session %q not found in %s", id, opencodeDBPath())
	}
	msgRows, _ := queryRows(db, "SELECT id, time_created, data FROM message WHERE session_id = ? ORDER BY time_created, id", id)
	partRows, _ := queryRows(db, "SELECT message_id, time_created, data FROM part WHERE session_id = ? ORDER BY time_created, id", id)
	partsByMsg := map[string][]any{}
	for _, p := range partRows {
		mid, _ := p["message_id"].(string)
		partsByMsg[mid] = append(partsByMsg[mid], safeParse(p["data"]))
	}
	messages := make([]any, 0, len(msgRows))
	for _, mr := range msgRows {
		mid, _ := mr["id"].(string)
		messages = append(messages, map[string]any{
			"id": mr["id"], "time_created": mr["time_created"],
			"data": safeParse(mr["data"]), "parts": partsByMsg[mid],
		})
	}
	return Input{Session: sessions[0], Messages: messages}, nil
}

var hermesMessageCols = "role, content, tool_call_id, tool_calls, tool_name, timestamp, token_count, finish_reason, reasoning, reasoning_content"

func latestHermes(cwd string) (*Found, bool) {
	db, err := openDB(hermesDBPath())
	if err != nil {
		return nil, false
	}
	rows, err := queryRows(db, "SELECT id, started_at FROM sessions WHERE cwd = ? ORDER BY started_at DESC LIMIT 1", cwd)
	db.Close()
	if err != nil || len(rows) == 0 {
		return nil, false
	}
	id, _ := rows[0]["id"].(string)
	// hermes started_at is seconds; scale to ns.
	found := Found{Harness: "hermes", Recency: int64(numOr(rows[0]["started_at"], 0)) * 1e9, load: func() (Input, error) { return loadHermes(id) }}
	return &found, true
}

func loadHermes(id string) (Input, error) {
	db, err := openDB(hermesDBPath())
	if err != nil {
		return Input{}, err
	}
	defer db.Close()
	sessions, err := queryRows(db, "SELECT * FROM sessions WHERE id = ?", id)
	if err != nil {
		return Input{}, err
	}
	if len(sessions) == 0 {
		return Input{}, fmt.Errorf("hermes session %q not found in %s", id, hermesDBPath())
	}
	messages, _ := queryRows(db, "SELECT "+hermesMessageCols+" FROM messages WHERE session_id = ? ORDER BY id", id)
	msgs := make([]any, len(messages))
	for i, mm := range messages {
		msgs[i] = mm
	}
	return Input{Session: sessions[0], Messages: msgs}, nil
}

// safeParse: a JSON-string column → its parsed value; already-parsed or
// non-JSON stays as-is.
func safeParse(v any) any {
	s, ok := v.(string)
	if !ok {
		return v
	}
	var out any
	if json.Unmarshal([]byte(s), &out) != nil {
		return map[string]any{}
	}
	return out
}

/* ------------------------------------------- newest-anywhere discovery */

// Elsewhere is the newest session on the machine regardless of project —
// what `slink import`'s empty state points at so "nothing here" never
// reads as "nothing anywhere".
type Elsewhere struct {
	Harness string
	Dir     string // project directory the session belongs to ("" if unknown)
	File    string // transcript path for file-based harnesses ("" for DBs)
	Title   string // best-effort; "" when the harness can't say cheaply
	Recency int64  // ns since epoch
}

// NewestAnywhere finds the machine's most recent session for one harness
// ("" = across all). Bounded work: one readdir per project dir for the
// file-based harnesses, one LIMIT 1 query for the DB-backed ones.
func NewestAnywhere(harness string) (*Elsewhere, bool) {
	finders := map[string]func() (*Elsewhere, bool){
		"claude-code": anywhereClaude,
		"pi":          anywherePi,
		"codex":       anywhereCodex,
		"opencode":    anywhereOpencode,
		"hermes":      anywhereHermes,
	}
	if harness != "" {
		if fn := finders[harness]; fn != nil {
			return fn()
		}
		return nil, false
	}
	var best *Elsewhere
	for _, h := range []string{"claude-code", "pi", "codex", "opencode", "hermes"} {
		if e, ok := finders[h](); ok && (best == nil || e.Recency > best.Recency) {
			best = e
		}
	}
	return best, best != nil
}

// newestUnderProjects scans base/*/ for the newest transcript across every
// project directory.
func newestUnderProjects(base string) (string, int64) {
	entries, err := os.ReadDir(base)
	if err != nil {
		return "", 0
	}
	best, bestT := "", int64(-1)
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		if f, t := newestFile(filepath.Join(base, e.Name())); f != "" && t > bestT {
			best, bestT = f, t
		}
	}
	return best, bestT
}

// peekTranscript reads the first lines of a transcript for a cwd and a
// best-effort title without loading the whole file.
func peekTranscript(file string, maxLines int) (cwd, title string) {
	f, err := os.Open(file)
	if err != nil {
		return "", ""
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 0, 64*1024), 16*1024*1024)
	var users []map[string]any
	for n := 0; sc.Scan() && n < maxLines; n++ {
		var e map[string]any
		if json.Unmarshal(sc.Bytes(), &e) != nil {
			continue
		}
		if cwd == "" {
			if c := strOr(e["cwd"], ""); c != "" {
				cwd = c
			} else if p := m(e["payload"]); p != nil { // codex session_meta
				cwd = strOr(p["cwd"], "")
			}
		}
		if t := strOr(e["type"], ""); t == "summary" && title == "" {
			title = strOr(e["summary"], "")
		} else if t == "user" {
			users = append(users, e)
		}
	}
	if title == "" {
		title = ccFirstUserText(users)
	}
	return cwd, title
}

func anywhereClaude() (*Elsewhere, bool) {
	f, t := newestUnderProjects(filepath.Join(home(), ".claude", "projects"))
	if f == "" {
		return nil, false
	}
	cwd, title := peekTranscript(f, 40)
	return &Elsewhere{Harness: "claude-code", Dir: cwd, File: f, Title: title, Recency: t}, true
}

func anywherePi() (*Elsewhere, bool) {
	f, t := newestUnderProjects(filepath.Join(home(), ".pi", "agent", "sessions"))
	if f == "" {
		return nil, false
	}
	cwd, title := peekTranscript(f, 40)
	return &Elsewhere{Harness: "pi", Dir: cwd, File: f, Title: title, Recency: t}, true
}

func anywhereCodex() (*Elsewhere, bool) {
	dir := codexSessionsDir()
	best, bestT := "", int64(-1)
	filepath.WalkDir(dir, func(p string, d os.DirEntry, err error) error {
		if err != nil || d.IsDir() || !strings.HasSuffix(p, ".jsonl") {
			return nil
		}
		if info, err := d.Info(); err == nil && info.ModTime().UnixNano() > bestT {
			best, bestT = p, info.ModTime().UnixNano()
		}
		return nil
	})
	if best == "" {
		return nil, false
	}
	cwd, title := peekTranscript(best, 40)
	return &Elsewhere{Harness: "codex", Dir: cwd, File: best, Title: title, Recency: bestT}, true
}

func anywhereOpencode() (*Elsewhere, bool) {
	db, err := openDB(opencodeDBPath())
	if err != nil {
		return nil, false
	}
	rows, err := queryRows(db, "SELECT directory, title, time_created FROM session ORDER BY time_created DESC LIMIT 1")
	db.Close()
	if err != nil || len(rows) == 0 {
		return nil, false
	}
	return &Elsewhere{
		Harness: "opencode",
		Dir:     strOr(rows[0]["directory"], ""),
		Title:   strOr(rows[0]["title"], ""),
		Recency: int64(numOr(rows[0]["time_created"], 0)) * 1e6,
	}, true
}

func anywhereHermes() (*Elsewhere, bool) {
	db, err := openDB(hermesDBPath())
	if err != nil {
		return nil, false
	}
	rows, err := queryRows(db, "SELECT cwd, title, started_at FROM sessions ORDER BY started_at DESC LIMIT 1")
	db.Close()
	if err != nil || len(rows) == 0 {
		return nil, false
	}
	return &Elsewhere{
		Harness: "hermes",
		Dir:     strOr(rows[0]["cwd"], ""),
		Title:   strOr(rows[0]["title"], ""),
		Recency: int64(numOr(rows[0]["started_at"], 0)) * 1e9,
	}, true
}

// LatestByID loads a specific DB-backed session by id.
func LatestByID(harness, id string) (*Found, bool) {
	switch harness {
	case "opencode":
		f := Found{Harness: "opencode", load: func() (Input, error) { return loadOpencode(id) }}
		return &f, true
	case "hermes":
		f := Found{Harness: "hermes", load: func() (Input, error) { return loadHermes(id) }}
		return &f, true
	}
	return nil, false
}

// LoadFile builds Input from an explicit transcript path (bare `slink
// import <file>`), sniffing which file-based harness it is.
func LoadFile(file string) (Input, string, error) {
	lines, err := readLines(file)
	if err != nil {
		return Input{}, "", err
	}
	in := Input{Lines: lines, Fallback: strings.TrimSuffix(filepath.Base(file), ".jsonl")}
	return in, sniff(lines), nil
}

// sniff peeks the first entries to pick a file-based harness.
func sniff(lines []string) string {
	var peek []map[string]any
	for _, l := range lines {
		var v map[string]any
		if json.Unmarshal([]byte(l), &v) == nil {
			peek = append(peek, v)
		}
		if len(peek) >= 20 {
			break
		}
	}
	for _, e := range peek {
		if strOr(e["type"], "") == "session_meta" {
			return "codex"
		}
		if strOr(e["type"], "") == "response_item" && e["payload"] != nil {
			return "codex"
		}
	}
	for _, e := range peek {
		if t := strOr(e["type"], ""); t == "session" || t == "message" {
			return "pi"
		}
	}
	for _, e := range peek {
		if t := strOr(e["type"], ""); t == "user" || t == "assistant" {
			return "claude-code"
		}
	}
	return ""
}
