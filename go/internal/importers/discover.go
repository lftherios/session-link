package importers

import (
	"bufio"
	"bytes"
	"database/sql"
	"encoding/json"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf16"

	"github.com/klauspost/compress/zstd"
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

// claudeProjectsDir is where Claude Code keeps one directory of transcripts
// per project. CLAUDE_CONFIG_DIR moves its whole configuration directory.
func claudeProjectsDir() string {
	if dir := os.Getenv("CLAUDE_CONFIG_DIR"); dir != "" {
		return filepath.Join(dir, "projects")
	}
	return filepath.Join(home(), ".claude", "projects")
}

// claudeSlugLimit is the longest directory name Claude Code derives from a
// path alone. A longer one is cut here and ends in a hash of the whole path.
const claudeSlugLimit = 200

// claudeSlug names a project directory the way Claude Code does: every UTF-16
// code unit other than an ASCII letter or digit becomes "-".
func claudeSlug(cwd string) string {
	var slug strings.Builder
	for _, unit := range utf16.Encode([]rune(cwd)) {
		if (unit >= 'a' && unit <= 'z') || (unit >= 'A' && unit <= 'Z') || (unit >= '0' && unit <= '9') {
			slug.WriteByte(byte(unit))
		} else {
			slug.WriteByte('-')
		}
	}
	return slug.String()
}

// claudeTranscripts lists the transcripts Claude Code keeps under base for
// one working directory. A long path is found by its cut name, since the hash
// that follows depends on the runtime Claude Code was built for; projects can
// share that name, so each transcript must also record cwd as its directory.
func claudeTranscripts(base, cwd string) []string {
	slug := claudeSlug(cwd)
	if len(slug) <= claudeSlugLimit {
		return transcriptsIn(filepath.Join(base, slug))
	}
	entries, _ := os.ReadDir(base)
	var files []string
	for _, entry := range entries {
		if !entry.IsDir() || !strings.HasPrefix(entry.Name(), slug[:claudeSlugLimit]+"-") {
			continue
		}
		for _, file := range transcriptsIn(filepath.Join(base, entry.Name())) {
			if recorded, _ := peekTranscript(file, 40); recorded == cwd {
				files = append(files, file)
			}
		}
	}
	return files
}

// ClaudeTranscript resolves a pasted session ID for one working directory
// (Claude Code names transcripts <uuid>.jsonl).
func ClaudeTranscript(cwd, id string) (string, bool) {
	for _, file := range claudeTranscripts(claudeProjectsDir(), cwd) {
		if filepath.Base(file) == id+".jsonl" {
			return file, true
		}
	}
	return "", false
}

// transcriptsIn lists the .jsonl files directly inside dir.
func transcriptsIn(dir string) []string {
	entries, _ := os.ReadDir(dir)
	var files []string
	for _, entry := range entries {
		if !entry.IsDir() && strings.HasSuffix(entry.Name(), ".jsonl") {
			files = append(files, filepath.Join(dir, entry.Name()))
		}
	}
	return files
}

// piSessionsDir is where pi keeps one directory of transcripts per project.
// PI_CODING_AGENT_DIR moves its whole agent directory.
func piSessionsDir() string {
	if dir := os.Getenv("PI_CODING_AGENT_DIR"); dir != "" {
		return filepath.Join(expandHome(dir), "sessions")
	}
	return filepath.Join(home(), ".pi", "agent", "sessions")
}

// piProjectDir names a project's directory under sessions the way pi does.
func piProjectDir(sessions, cwd string) string {
	enc := strings.NewReplacer("/", "-", `\`, "-", ":", "-").Replace(strings.TrimLeft(cwd, `/\`))
	return filepath.Join(sessions, "--"+enc+"--")
}

// piSessionDir is the single directory pi keeps sessions in when it is given
// one: PI_CODING_AGENT_SESSION_DIR, else sessionDir in the settings.json of
// the project or of the agent directory. "" means the default layout.
func piSessionDir(agentDir, cwd string) string {
	dir := os.Getenv("PI_CODING_AGENT_SESSION_DIR")
	for _, settings := range []string{filepath.Join(cwd, ".pi", "settings.json"), filepath.Join(agentDir, "settings.json")} {
		if dir != "" {
			break
		}
		var parsed map[string]any
		if raw, err := os.ReadFile(settings); err == nil && json.Unmarshal(raw, &parsed) == nil {
			dir = strOr(parsed["sessionDir"], "")
		}
	}
	if dir == "" {
		return ""
	}
	if dir = expandHome(dir); !filepath.IsAbs(dir) {
		dir = filepath.Join(cwd, dir) // pi resolves it from the working directory
	}
	return dir
}

// piTranscripts lists pi's transcripts for one working directory: those in
// the project's own directory under sessions, and those a configured session
// directory holds for it. That directory is shared by every project using
// it, so each transcript's header says which one it belongs to.
func piTranscripts(sessions, cwd string) []string {
	files := transcriptsIn(piProjectDir(sessions, cwd))
	if shared := piSessionDir(filepath.Dir(sessions), cwd); shared != "" {
		for _, file := range transcriptsIn(shared) {
			if strOr(piHeader(file)["cwd"], "") == cwd {
				files = append(files, file)
			}
		}
	}
	return files
}

// ompProfileName is the form omp accepts for a profile name.
var ompProfileName = regexp.MustCompile(`^[a-z0-9][a-z0-9._-]{0,63}$`)

// ompSessionsDir is where omp keeps one directory of transcripts per project,
// resolved the way omp resolves it: PI_CONFIG_DIR names its directory under
// home (".omp"), OMP_PROFILE (or PI_PROFILE when that is unset) selects a
// profile inside it, and PI_CODING_AGENT_DIR moves the default profile's agent
// directory. Data that omp has moved under $XDG_DATA_HOME/omp is read there.
func ompSessionsDir() string {
	root := os.Getenv("PI_CONFIG_DIR")
	if root == "" {
		root = ".omp"
	}
	root = filepath.Join(home(), root)
	profile, set := os.LookupEnv("OMP_PROFILE")
	if !set {
		profile = os.Getenv("PI_PROFILE")
	}
	if profile = strings.TrimSpace(profile); profile == "default" || !ompProfileName.MatchString(profile) || strings.HasSuffix(profile, ".") {
		profile = ""
	}
	xdg := os.Getenv("XDG_DATA_HOME")
	if xdg != "" {
		xdg = filepath.Join(xdg, "omp")
	}
	if profile != "" {
		root = filepath.Join(root, "profiles", profile)
		if xdg != "" {
			xdg = filepath.Join(xdg, "profiles", profile)
		}
	} else if dir := os.Getenv("PI_CODING_AGENT_DIR"); dir != "" {
		return filepath.Join(dir, "sessions")
	}
	if info, err := os.Stat(xdg); xdg != "" && err == nil && info.IsDir() {
		return filepath.Join(xdg, "sessions")
	}
	return filepath.Join(root, "agent", "sessions")
}

// ompTranscripts lists omp's transcripts for one working directory. omp names
// a project by its path relative to the home directory ("-code-app") or to
// the temp directory ("-tmp-build"), after resolving symlinks, and only
// otherwise by its whole path, the way pi does. A project last used before
// that naming still has the whole-path name, until omp runs there again.
func ompTranscripts(sessions, cwd string) []string {
	resolved := func(path string) string {
		if real, err := filepath.EvalSymlinks(path); err == nil {
			return real
		}
		return filepath.Clean(path)
	}
	inside := func(root string, path string) (string, bool) {
		rel, err := filepath.Rel(root, path)
		if err != nil || rel == ".." || strings.HasPrefix(rel, ".."+string(os.PathSeparator)) || filepath.IsAbs(rel) {
			return "", false
		}
		if rel == "." {
			rel = ""
		}
		return rel, true
	}
	relative := func(prefix, rel string) string {
		if rel == "" {
			return filepath.Join(sessions, prefix)
		}
		return filepath.Join(sessions, strings.TrimSuffix(prefix, "-")+"-"+strings.NewReplacer("/", "-", `\`, "-", ":", "-").Replace(rel))
	}
	canonical := resolved(cwd)
	dirs := []string{piProjectDir(sessions, canonical)}
	if rel, ok := inside(resolved(home()), canonical); ok {
		dirs[0] = relative("-", rel)
	} else if rel, ok := inside(resolved(os.TempDir()), canonical); ok {
		dirs[0] = relative("-tmp", rel)
	}
	for _, older := range []string{piProjectDir(sessions, canonical), piProjectDir(sessions, filepath.Clean(cwd))} {
		if !slices.Contains(dirs, older) {
			dirs = append(dirs, older)
		}
	}
	var files []string
	for _, dir := range dirs {
		files = append(files, transcriptsIn(dir)...)
	}
	return files
}

// piHeader finds the session header among the first lines of a pi or omp
// transcript. omp writes its title line ahead of it.
func piHeader(file string) map[string]any {
	f, err := os.Open(file)
	if err != nil {
		return nil
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 0, 64*1024), 16*1024*1024)
	for n := 0; sc.Scan() && n < 5; n++ {
		var e map[string]any
		if json.Unmarshal(sc.Bytes(), &e) == nil && strOr(e["type"], "") == "session" {
			return e
		}
	}
	return nil
}

// ompSessionTitle finds the title omp gave a session. omp keeps the current
// one in the transcript's first line, which it rewrites in place; the header
// and the latest title change are where an older transcript has it.
func ompSessionTitle(file string) string {
	if title := strings.TrimSpace(strOr(firstJSONLine(file)["title"], "")); title != "" {
		return title
	}
	lines := tailLines(file)
	for i := len(lines) - 1; i >= 0; i-- {
		if !bytes.Contains(lines[i], []byte(`"title_change"`)) {
			continue
		}
		var e map[string]any
		if json.Unmarshal(lines[i], &e) == nil && strOr(e["type"], "") == "title_change" {
			if title := strings.TrimSpace(strOr(e["title"], "")); title != "" {
				return title
			}
		}
	}
	return strings.TrimSpace(strOr(piHeader(file)["title"], ""))
}

// expandHome resolves a leading ~ the way the harnesses do for their paths.
func expandHome(path string) string {
	if path == "~" {
		return home()
	}
	if strings.HasPrefix(path, "~/") {
		return filepath.Join(home(), path[2:])
	}
	return path
}

func codexSessionsDir() string {
	if h := os.Getenv("CODEX_HOME"); h != "" {
		return filepath.Join(h, "sessions")
	}
	return filepath.Join(home(), ".codex", "sessions")
}

// opencodeDBPath is opencode's session store. OPENCODE_DB names another one,
// by absolute path or by file name inside opencode's data directory.
func opencodeDBPath() string {
	dataHome := os.Getenv("XDG_DATA_HOME")
	if dataHome == "" {
		dataHome = filepath.Join(home(), ".local", "share")
	}
	name := "opencode.db"
	if db := os.Getenv("OPENCODE_DB"); db != "" && db != ":memory:" {
		if filepath.IsAbs(db) {
			return db
		}
		name = db
	}
	return filepath.Join(dataHome, "opencode", name)
}

// hermesDBPath is the session store a plain `hermes` would open, resolved the
// way Hermes resolves its home: HERMES_HOME when it names a profile, else the
// profile the active_profile file in the Hermes root selects, else the root.
// HERMES_STATE_DB, slink's own override, names the file outright.
func hermesDBPath() string {
	if p := os.Getenv("HERMES_STATE_DB"); p != "" {
		return p
	}
	root := filepath.Join(home(), ".hermes")
	if env := os.Getenv("HERMES_HOME"); env != "" {
		if filepath.Base(filepath.Dir(env)) == "profiles" {
			return filepath.Join(env, "state.db")
		}
		root = env
	}
	if name, err := os.ReadFile(filepath.Join(root, "active_profile")); err == nil {
		profile := strings.TrimSpace(string(name))
		dir := filepath.Join(root, "profiles", profile)
		if info, err := os.Stat(dir); profile != "" && profile != "default" && !strings.ContainsAny(profile, `/\`) && err == nil && info.IsDir() {
			return filepath.Join(dir, "state.db")
		}
	}
	return filepath.Join(root, "state.db")
}

/* -------------------------------------------------------- file loaders */

// zstdMagic opens every Zstandard frame.
var zstdMagic = []byte{0x28, 0xB5, 0x2F, 0xFD}

// readTranscript reads a transcript file whole. dsh compresses its logs, as
// one Zstandard frame per write, and those are decoded here. A last frame
// that was cut off mid-write gives up the lines it completed, as it does in
// dsh.
func readTranscript(file string) ([]byte, error) {
	raw, err := os.ReadFile(file)
	if err != nil || !bytes.HasPrefix(raw, zstdMagic) {
		return raw, err
	}
	decoder, err := zstd.NewReader(bytes.NewReader(raw), zstd.WithDecoderConcurrency(1))
	if err != nil {
		return nil, err
	}
	defer decoder.Close()
	text, err := io.ReadAll(io.LimitReader(decoder, 1<<30))
	if err != nil {
		end := bytes.LastIndexByte(text, '\n')
		if end < 0 {
			return nil, fmt.Errorf("%s: %w", file, err)
		}
		text = text[:end+1]
	}
	return text, nil
}

func readLines(file string) ([]string, error) {
	raw, err := readTranscript(file)
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
		if harness == "aider" {
			return aiderInput(file, "")
		}
		lines, err := readLines(file)
		if err != nil {
			return Input{}, err
		}
		return transcriptInput(file, harness, lines), nil
	}}
}

// transcriptInput is a transcript's lines with what its harness keeps beside
// them: the store its images are in, and the sessions of its sub-agents.
func transcriptInput(file, harness string, lines []string) Input {
	in := Input{Lines: lines, Fallback: strings.TrimSuffix(filepath.Base(file), ".jsonl"), Blobs: blobsBeside(file), Agents: fileAgents(harness, file)}
	if harness == "dsh" {
		in.Fallback, in.Blobs = filepath.Base(filepath.Dir(file)), dshAttachments(file)
	}
	return in
}

/* ----------------------------------------------------------- sub-agents
   Each harness keeps a sub-agent's work apart from the session that started
   it. These read it back, so the importer can nest it where it belongs. */

// fileAgents reads the sub-agents of a session kept in a transcript file.
func fileAgents(harness, transcript string) []Agent {
	switch harness {
	case "claude-code":
		return claudeAgents(transcript)
	case "omp":
		return ompAgents(transcript, blobsBeside(transcript), 0)
	case "codex":
		id := strOr(m(firstJSONLine(transcript)["payload"])["id"], "")
		return codexAgents(codexThreads(codexRoot(transcript)), id, 0)
	case "dsh":
		return dshAgents(transcript, 0)
	}
	return nil
}

// agentFiles lists the transcripts of a session's sub-agents, and of theirs.
// A preview made from the session depends on every one of them. threads is
// the index of Codex's sub-agent threads, which one walk builds for a whole
// listing.
func agentFiles(harness, transcript, thread string, threads map[string][]codexThread) []string {
	var files []string
	switch harness {
	case "claude-code":
		files = transcriptsIn(filepath.Join(strings.TrimSuffix(transcript, ".jsonl"), "subagents"))
	case "omp":
		for _, file := range transcriptsIn(strings.TrimSuffix(transcript, ".jsonl")) {
			files = append(append(files, file), agentFiles(harness, file, "", nil)...)
		}
	case "codex":
		var collect func(id string, depth int)
		collect = func(id string, depth int) {
			for _, child := range threads[id] {
				if files = append(files, child.file); depth < 8 {
					collect(child.id, depth+1)
				}
			}
		}
		collect(thread, 0)
	case "dsh":
		for _, child := range dshChildren(transcript) {
			files = append(append(files, child.file), agentFiles(harness, child.file, "", nil)...)
		}
	}
	return files
}

// claudeAgents reads a Claude Code session's sub-agents. Claude Code writes
// each one's transcript to subagents/agent-<id>.jsonl, in a directory named
// after the session, with a .meta.json that names the tool call that started
// it. A sub-agent's own sub-agents are in the same directory, one spawn
// deeper, so the shallower ones come first.
func claudeAgents(transcript string) []Agent {
	var agents []Agent
	depth := map[string]float64{}
	for _, file := range transcriptsIn(filepath.Join(strings.TrimSuffix(transcript, ".jsonl"), "subagents")) {
		lines, err := readLines(file)
		if err != nil {
			continue
		}
		id := strings.TrimPrefix(strings.TrimSuffix(filepath.Base(file), ".jsonl"), "agent-")
		var meta map[string]any
		if raw, err := os.ReadFile(strings.TrimSuffix(file, ".jsonl") + ".meta.json"); err == nil {
			json.Unmarshal(raw, &meta)
		}
		depth[id] = numOr(meta["spawnDepth"], 1)
		agents = append(agents, Agent{ID: id, Call: strOr(meta["toolUseId"], ""), Name: strOr(meta["description"], strOr(meta["agentType"], "")),
			Input: Input{Lines: lines, Fallback: id}})
	}
	sort.SliceStable(agents, func(i, j int) bool { return depth[agents[i].ID] < depth[agents[j].ID] })
	return agents
}

// ompAgents reads an omp session's sub-agents. omp writes each one's
// transcript, named after the task, into a directory named after the
// session's own file; a sub-agent's sub-agents are beside it the same way.
func ompAgents(transcript, blobs string, depth int) []Agent {
	var agents []Agent
	for _, file := range transcriptsIn(strings.TrimSuffix(transcript, ".jsonl")) {
		lines, err := readLines(file)
		if err != nil || depth > 8 {
			continue
		}
		id := strings.TrimSuffix(filepath.Base(file), ".jsonl")
		agents = append(agents, Agent{ID: id, Name: id, Input: Input{Lines: lines, Fallback: id, Blobs: blobs, Agents: ompAgents(file, blobs, depth+1)}})
	}
	return agents
}

type codexThread struct{ id, file, path, nickname string }

// codexRoot is the directory to look in for a rollout's sub-agent threads:
// Codex's sessions directory when the rollout is in it, else the top of the
// year/month/day directories the rollout sits in, else its own directory.
func codexRoot(rollout string) string {
	if sessions := codexSessionsDir(); strings.HasPrefix(rollout, sessions+string(os.PathSeparator)) {
		return sessions
	}
	dir := filepath.Dir(rollout)
	dated := dir
	for range 3 {
		if _, err := strconv.Atoi(filepath.Base(dated)); err != nil {
			return dir
		}
		dated = filepath.Dir(dated)
	}
	return dated
}

// codexThreads indexes the sub-agent threads under root by the thread that
// spawned each. Codex writes a sub-agent's rollout beside every other one;
// its header names its parent thread and the task path it was given.
func codexThreads(root string) map[string][]codexThread {
	threads := map[string][]codexThread{}
	filepath.WalkDir(root, func(file string, entry os.DirEntry, err error) error {
		if err != nil || entry.IsDir() || !strings.HasSuffix(file, ".jsonl") {
			return nil
		}
		meta := m(firstJSONLine(file)["payload"])
		if parent := strOr(meta["parent_thread_id"], ""); parent != "" && codexAuxiliaryThread(meta) {
			threads[parent] = append(threads[parent], codexThread{strOr(meta["id"], ""), file, strOr(meta["agent_path"], ""), strOr(meta["agent_nickname"], "")})
		}
		return nil
	})
	return threads
}

// codexAgents reads the sub-agents of one thread, and theirs.
func codexAgents(threads map[string][]codexThread, id string, depth int) []Agent {
	var agents []Agent
	for _, thread := range threads[id] {
		lines, err := readLines(thread.file)
		if err != nil || id == "" || depth > 8 {
			continue
		}
		name := filepath.Base(thread.path)
		if thread.path == "" {
			name = thread.nickname
		}
		agents = append(agents, Agent{ID: thread.path, Name: name,
			Input: Input{Lines: lines, Fallback: strings.TrimSuffix(filepath.Base(thread.file), ".jsonl"), Agents: codexAgents(threads, thread.id, depth+1)}})
	}
	return agents
}

// blobsBeside is where omp keeps the images of a transcript: its blob store
// sits next to the sessions directory that holds the project's directory.
func blobsBeside(transcript string) string {
	return filepath.Join(filepath.Dir(filepath.Dir(filepath.Dir(transcript))), "blobs")
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
		"omp":         latestOmp,
		"codex":       latestCodex,
		"opencode":    latestOpencode,
		"hermes":      latestHermes,
		"dsh":         latestDsh,
		"aider":       latestAider,
	}
	// A command an agent runs in its own shell tool is about the session
	// running it, whichever was written to last.
	if active, id, ok := ActiveSession(); ok && (harness == "" || harness == active) {
		if file, ok := dshByID(dshSessionsDir(), id); ok {
			if info, err := os.Stat(file); err == nil {
				found := fileInput(file, active, info.ModTime().UnixNano())
				return &found, true
			}
		}
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
	for _, h := range []string{"claude-code", "pi", "omp", "codex", "opencode", "hermes", "dsh", "aider"} {
		if f, ok := try[h](cwd); ok {
			if best == nil || f.Recency > best.Recency {
				best = f
			}
		}
	}
	return best, best != nil
}

func latestClaude(cwd string) (*Found, bool) {
	return newestOf(claudeTranscripts(claudeProjectsDir(), cwd), "claude-code")
}

func latestPi(cwd string) (*Found, bool) {
	return newestOf(piTranscripts(piSessionsDir(), cwd), "pi")
}

func latestOmp(cwd string) (*Found, bool) {
	return newestOf(ompTranscripts(ompSessionsDir(), cwd), "omp")
}

// newestOf picks the most recently written transcript.
func newestOf(files []string, harness string) (*Found, bool) {
	best, bestT := "", int64(-1)
	for _, file := range files {
		if info, err := os.Stat(file); err == nil && info.ModTime().UnixNano() > bestT {
			best, bestT = file, info.ModTime().UnixNano()
		}
	}
	if best == "" {
		return nil, false
	}
	found := fileInput(best, harness, bestT)
	return &found, true
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
			meta := m(head["payload"])
			if strOr(meta["cwd"], "") == cwd && !codexAuxiliaryThread(meta) {
				found := fileInput(r.path, "codex", r.mod)
				return &found, true
			}
		}
	}
	return nil, false
}

// codexAuxiliaryThread reports a rollout for a thread the person did not
// start: a sub-agent, a review or compaction helper, or internal work such as
// memory consolidation. Codex writes each one beside the session that
// spawned it, with the same working directory.
func codexAuxiliaryThread(meta map[string]any) bool {
	source := m(meta["source"])
	return source["subagent"] != nil || source["internal"] != nil
}

func firstJSONLine(file string) map[string]any {
	f, err := os.Open(file)
	if err != nil {
		return nil
	}
	defer f.Close()
	var source io.Reader = f
	if head := make([]byte, len(zstdMagic)); strings.HasSuffix(file, ".zstd") {
		if _, err := io.ReadFull(f, head); err != nil || !bytes.Equal(head, zstdMagic) {
			return nil
		}
		f.Seek(0, io.SeekStart)
		decoder, err := zstd.NewReader(f, zstd.WithDecoderConcurrency(1))
		if err != nil {
			return nil
		}
		defer decoder.Close()
		source = decoder
	}
	sc := bufio.NewScanner(source)
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
	rows, err := opencodeSessions(db, cwd, "", 1)
	db.Close()
	if err != nil || len(rows) == 0 {
		return nil, false
	}
	id, _ := rows[0]["id"].(string)
	// opencode times are ms; scale to ns to compare with file mtimes.
	found := Found{Harness: "opencode", Recency: int64(numOr(rows[0]["active_at"], 0)) * 1e6, load: func() (Input, error) { return loadOpencode(id) }}
	return &found, true
}

// tableColumns names a table's columns, so a query can use what this version
// of a harness records without failing on a store that predates it.
func tableColumns(db *sql.DB, table string) map[string]bool {
	columns := map[string]bool{}
	rows, err := queryRows(db, "PRAGMA table_info("+table+")")
	if err != nil {
		return columns
	}
	for _, row := range rows {
		columns[strOr(row["name"], "")] = true
	}
	return columns
}

// opencodeSessions lists the sessions opencode recorded for one directory,
// most recently active first: active_at is when the session last changed,
// which is how opencode orders its own list. A session with a parent is a
// sub-agent's task, which opencode leaves out; only an explicit id reaches one.
func opencodeSessions(db *sql.DB, dir, id string, limit int) ([]map[string]any, error) {
	columns := tableColumns(db, "session")
	active := "time_created"
	if columns["time_updated"] {
		active = "COALESCE(time_updated, time_created)"
	}
	query, args := "SELECT id, title, time_created, "+active+" AS active_at FROM session WHERE directory = ?", []any{dir}
	if id != "" {
		query += " AND id = ?"
		args = append(args, id)
	} else if columns["parent_id"] {
		query += " AND parent_id IS NULL"
	}
	return queryRows(db, query+" ORDER BY active_at DESC, id LIMIT ?", append(args, limit)...)
}

func loadOpencode(id string) (Input, error) {
	return loadOpencodeAt(opencodeDBPath(), id)
}

func loadOpencodeAt(path, id string) (Input, error) {
	db, err := openDB(path)
	if err != nil {
		return Input{}, err
	}
	defer db.Close()
	return loadOpencodeSession(db, path, id, 0)
}

func loadOpencodeSession(db *sql.DB, path, id string, depth int) (Input, error) {
	sessions, err := queryRows(db, "SELECT * FROM session WHERE id = ?", id)
	if err != nil {
		return Input{}, err
	}
	if len(sessions) == 0 {
		return Input{}, fmt.Errorf("opencode session %q not found in %s", id, path)
	}
	msgRows, err := queryRows(db, "SELECT id, time_created, data FROM message WHERE session_id = ? ORDER BY time_created, id", id)
	if err != nil {
		return Input{}, fmt.Errorf("cannot read opencode messages: %w", err)
	}
	partRows, err := queryRows(db, "SELECT message_id, time_created, data FROM part WHERE session_id = ? ORDER BY time_created, id", id)
	if err != nil {
		return Input{}, fmt.Errorf("cannot read opencode message parts: %w", err)
	}
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
	in := Input{Session: sessions[0], Messages: messages}
	// A session with this one as parent is a sub-agent's task.
	if tableColumns(db, "session")["parent_id"] && depth < 8 {
		children, _ := queryRows(db, "SELECT id, title FROM session WHERE parent_id = ? ORDER BY time_created, id", id)
		for _, child := range children {
			if agent, err := loadOpencodeSession(db, path, strOr(child["id"], ""), depth+1); err == nil {
				in.Agents = append(in.Agents, Agent{ID: strOr(child["id"], ""), Name: strOr(child["title"], ""), Input: agent})
			}
		}
	}
	return in, nil
}

var hermesMessageCols = "role, content, tool_call_id, tool_calls, tool_name, timestamp, token_count, finish_reason, reasoning, reasoning_content"

func latestHermes(cwd string) (*Found, bool) {
	db, err := openDB(hermesDBPath())
	if err != nil {
		return nil, false
	}
	rows, err := hermesSessions(db, cwd, "", 1)
	db.Close()
	if err != nil || len(rows) == 0 {
		return nil, false
	}
	id, _ := rows[0]["id"].(string)
	// hermes times are seconds; scale to ns.
	found := Found{Harness: "hermes", Recency: int64(numOr(rows[0]["active_at"], 0)) * 1e9, load: func() (Input, error) { return loadHermes(id) }}
	return &found, true
}

// hermesSessions lists the sessions Hermes recorded for one directory, most
// recently active first: active_at is the time of a session's last message.
// Hermes links a session to a parent in three cases, told apart by how the
// parent ended, and this follows the rules of its own session list:
//
//   - A continuation starts after context compression ended its parent. Hermes
//     does not always record a directory for it, so it takes its parent's.
//   - A branch starts after the parent ended as branched, or carries the
//     branch marker. Both are the person's own sessions and are listed.
//   - Anything else started while its parent was live: a sub-agent or a
//     background review. Only an explicit id reaches one.
func hermesSessions(db *sql.DB, dir, id string, limit int) ([]map[string]any, error) {
	columns := tableColumns(db, "sessions")
	active := "s.started_at"
	if messages := tableColumns(db, "messages"); messages["session_id"] && messages["timestamp"] {
		active = "COALESCE((SELECT MAX(m.timestamp) FROM messages m WHERE m.session_id = s.id), s.started_at)"
	}
	fields, order := "s.id, s.title, s.started_at, "+active+" AS active_at", " ORDER BY active_at DESC, s.id LIMIT ?"
	if !columns["parent_session_id"] || !columns["end_reason"] || !columns["ended_at"] {
		query, args := "SELECT "+fields+" FROM sessions s WHERE s.cwd = ?", []any{dir}
		if id != "" {
			query += " AND s.id = ?"
			args = append(args, id)
		}
		return queryRows(db, query+order, append(args, limit)...)
	}
	after := func(reasons string) string {
		return "EXISTS (SELECT 1 FROM sessions p WHERE p.id = s.parent_session_id AND p.end_reason IN (" + reasons + ") AND s.started_at >= p.ended_at)"
	}
	query := `WITH RECURSIVE here(id) AS (
		SELECT id FROM sessions WHERE cwd = ?
		UNION
		SELECT s.id FROM sessions s JOIN here ON s.parent_session_id = here.id
		WHERE (s.cwd IS NULL OR s.cwd = '') AND ` + after("'compression'") + `)
		SELECT ` + fields + ` FROM sessions s WHERE s.id IN (SELECT id FROM here)`
	args := []any{dir}
	if id != "" {
		query += " AND s.id = ?"
		args = append(args, id)
	} else {
		own := "s.parent_session_id IS NULL OR " + after("'compression', 'branched'")
		if columns["model_config"] {
			own += " OR CASE WHEN json_valid(s.model_config) THEN json_extract(s.model_config, '$._branched_from') END IS NOT NULL"
		}
		query += " AND (" + own + ")"
	}
	return queryRows(db, query+order, append(args, limit)...)
}

func loadHermes(id string) (Input, error) {
	return loadHermesAt(hermesDBPath(), id)
}

func loadHermesAt(path, id string) (Input, error) {
	db, err := openDB(path)
	if err != nil {
		return Input{}, err
	}
	defer db.Close()
	return loadHermesSession(db, path, id, 0)
}

func loadHermesSession(db *sql.DB, path, id string, depth int) (Input, error) {
	sessions, err := queryRows(db, "SELECT * FROM sessions WHERE id = ?", id)
	if err != nil {
		return Input{}, err
	}
	if len(sessions) == 0 {
		return Input{}, fmt.Errorf("hermes session %q not found in %s", id, path)
	}
	query := "SELECT " + hermesMessageCols + " FROM messages WHERE session_id = ?"
	if tableColumns(db, "messages")["active"] {
		// Rewinding keeps the undone messages with active = 0. Hermes leaves
		// them out of the conversation it shows and resumes, and so does this.
		query += " AND active = 1"
	}
	messages, err := queryRows(db, query+" ORDER BY id", id)
	if err != nil {
		return Input{}, fmt.Errorf("cannot read hermes messages: %w", err)
	}
	msgs := make([]any, len(messages))
	for i, mm := range messages {
		msgs[i] = mm
	}
	in := Input{Session: sessions[0], Messages: msgs}
	// A session started while this one was live is a delegate's. One started
	// after this one ended is a continuation or a branch: a session of its own.
	if columns := tableColumns(db, "sessions"); columns["parent_session_id"] && columns["ended_at"] && depth < 8 {
		children, _ := queryRows(db, `SELECT s.id, s.title FROM sessions s JOIN sessions p ON p.id = s.parent_session_id
			WHERE p.id = ? AND (p.ended_at IS NULL OR s.started_at < p.ended_at) ORDER BY s.started_at, s.id`, id)
		for _, child := range children {
			if agent, err := loadHermesSession(db, path, strOr(child["id"], ""), depth+1); err == nil {
				in.Agents = append(in.Agents, Agent{ID: strOr(child["id"], ""), Name: strOr(child["title"], ""), Input: agent})
			}
		}
	}
	return in, nil
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
		"omp":         anywhereOmp,
		"codex":       anywhereCodex,
		"opencode":    anywhereOpencode,
		"hermes":      anywhereHermes,
		"dsh":         anywhereDsh,
	}
	if harness != "" {
		if fn := finders[harness]; fn != nil {
			return fn()
		}
		return nil, false
	}
	var best *Elsewhere
	for _, h := range []string{"claude-code", "pi", "omp", "codex", "opencode", "hermes", "dsh"} {
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
	cwd, name, prompt, _ := peekTitles(file, maxLines)
	if name != "" {
		return cwd, name
	}
	return cwd, prompt
}

// peekTitles reads the head of a transcript: its working directory, a title
// the harness recorded (Claude Code's own title or a summary), the first real
// prompt, and when the session started.
func peekTitles(file string, maxLines int) (cwd, name, prompt string, started time.Time) {
	f, err := os.Open(file)
	if err != nil {
		return "", "", "", time.Time{}
	}
	defer f.Close()
	sc := bufio.NewScanner(f)
	sc.Buffer(make([]byte, 0, 64*1024), 16*1024*1024)
	var users []map[string]any
	summary := ""
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
		if started.IsZero() {
			if t, err := time.Parse(time.RFC3339Nano, strOr(e["timestamp"], strOr(m(e["payload"])["timestamp"], ""))); err == nil {
				started = t
			}
		}
		switch t := strOr(e["type"], ""); {
		case t == "ai-title" && strOr(e["aiTitle"], "") != "":
			name = strOr(e["aiTitle"], "")
		case t == "summary" && summary == "":
			summary = strOr(e["summary"], "")
		case t == "user":
			users = append(users, e)
		case t == "message" && m(e["message"])["role"] == "user":
			users = append(users, map[string]any{"type": "user", "message": e["message"]})
		case t == "response_item" && m(e["payload"])["role"] == "user":
			msg := m(e["payload"])
			var text strings.Builder
			for _, p := range arr(msg["content"]) {
				part := m(p)
				if part["type"] == "input_text" || part["type"] == "text" {
					text.WriteString(strOr(part["text"], ""))
				}
			}
			users = append(users, map[string]any{"type": "user", "message": map[string]any{"content": text.String()}})
		}
	}
	if name == "" {
		name = summary
	}
	return cwd, name, ccFirstUserText(users), started
}

// tailLines returns the lines in the last 512 KB of a transcript, where a
// harness rewrites a session's title as work goes on.
func tailLines(file string) [][]byte {
	f, err := os.Open(file)
	if err != nil {
		return nil
	}
	defer f.Close()
	info, err := f.Stat()
	if err != nil {
		return nil
	}
	offset := info.Size() - 512*1024
	if offset < 0 {
		offset = 0
	}
	buf := make([]byte, info.Size()-offset)
	if _, err := f.ReadAt(buf, offset); err != nil && err != io.EOF {
		return nil
	}
	return bytes.Split(buf, []byte("\n"))
}

// latestAITitle finds the most recent title Claude Code recorded. It reads
// only the end of the transcript, where the title is rewritten as work goes on.
func latestAITitle(file string) string {
	lines := tailLines(file)
	for i := len(lines) - 1; i >= 0; i-- {
		if !bytes.Contains(lines[i], []byte(`"ai-title"`)) {
			continue
		}
		var e map[string]any
		if json.Unmarshal(lines[i], &e) == nil && strOr(e["type"], "") == "ai-title" {
			if title := strOr(e["aiTitle"], ""); title != "" {
				return title
			}
		}
	}
	return ""
}

// piSessionName finds the name the person gave a pi session. The latest
// session_info entry decides, and an empty name in it clears the name.
func piSessionName(file string) string {
	lines := tailLines(file)
	for i := len(lines) - 1; i >= 0; i-- {
		if !bytes.Contains(lines[i], []byte(`"session_info"`)) {
			continue
		}
		var e map[string]any
		if json.Unmarshal(lines[i], &e) == nil && strOr(e["type"], "") == "session_info" {
			return strings.TrimSpace(strOr(e["name"], ""))
		}
	}
	return ""
}

func anywhereClaude() (*Elsewhere, bool) {
	f, t := newestUnderProjects(claudeProjectsDir())
	if f == "" {
		return nil, false
	}
	cwd, title := peekTranscript(f, 40)
	return &Elsewhere{Harness: "claude-code", Dir: cwd, File: f, Title: title, Recency: t}, true
}

func anywherePi() (*Elsewhere, bool) {
	f, t := newestUnderProjects(piSessionsDir())
	if f == "" {
		return nil, false
	}
	cwd, title := peekTranscript(f, 40)
	return &Elsewhere{Harness: "pi", Dir: cwd, File: f, Title: title, Recency: t}, true
}

func anywhereOmp() (*Elsewhere, bool) {
	f, t := newestUnderProjects(ompSessionsDir())
	if f == "" {
		return nil, false
	}
	cwd, title := peekTranscript(f, 40)
	if recorded := ompSessionTitle(f); recorded != "" {
		title = recorded
	}
	return &Elsewhere{Harness: "omp", Dir: cwd, File: f, Title: title, Recency: t}, true
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
	if aiderChat(file) {
		in, err := aiderInput(file, "")
		return in, "aider", err
	}
	lines, err := readLines(file)
	if err != nil {
		return Input{}, "", err
	}
	harness := sniff(lines)
	return transcriptInput(file, harness, lines), harness, nil
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
	// dsh's header is a "session" line too; only it says whether the session
	// was seeded from another.
	for _, e := range peek[:min(len(peek), 1)] {
		if _, seeded := e["isSeeded"]; seeded && strOr(e["type"], "") == "session" {
			return "dsh"
		}
	}
	// omp's transcript is pi's with a title line of its own ahead of the header.
	for _, e := range peek {
		if _, slot := e["pad"]; slot && strOr(e["type"], "") == "title" {
			return "omp"
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

/* ------------------------------------------------------------------ dsh
   DeepSeek Harness keeps a directory per session, grouped by project, under
   sessions/ in its home. A session's directory holds its log, compressed
   unless dsh was configured not to, once for each format generation dsh has
   written it in. */

// dshSessionsDir is the sessions directory of dsh's home: DSH_HOME when it
// names one, ~/.dsh otherwise. A profile can be set to keep its logs
// somewhere else; that is not followed.
func dshSessionsDir() string {
	root := filepath.Join(home(), ".dsh")
	if env := os.Getenv("DSH_HOME"); strings.TrimSpace(env) != "" {
		root = expandHome(env)
		if abs, err := filepath.Abs(root); err == nil {
			root = abs
		}
	}
	return filepath.Join(root, "sessions")
}

// dshSafe is a character dsh writes into a path as it is.
func dshSafe(unit uint16) bool {
	return unit >= 'a' && unit <= 'z' || unit >= 'A' && unit <= 'Z' || unit >= '0' && unit <= '9' || unit == '.' || unit == '_' || unit == '-'
}

// dshSegment is a session id as dsh writes it for a directory name: every
// character it does not write as it is becomes ~ and its UTF-16 code unit.
func dshSegment(id string) string {
	switch id {
	case ".":
		return "~002E"
	case "..":
		return "~002E~002E"
	}
	var out strings.Builder
	for _, unit := range utf16.Encode([]rune(id)) {
		if dshSafe(unit) {
			out.WriteByte(byte(unit))
		} else {
			fmt.Fprintf(&out, "~%04X", unit)
		}
	}
	return out.String()
}

// dshProjectKey is the directory dsh groups a project's sessions under: the
// path with each run of separators as one dash and other characters escaped
// as in a session id, cut to 251 characters, between double dashes. Paths
// that differ only in what was replaced or cut share a directory, so it is
// each session's header that says which project it belongs to.
func dshProjectKey(cwd string) string {
	var readable strings.Builder
	separators := false
	for _, unit := range utf16.Encode([]rune(cwd)) {
		switch {
		case unit == '/' || unit == '\\' || unit == ':':
			if !separators {
				readable.WriteByte('-')
			}
			separators = true
			continue
		case dshSafe(unit):
			readable.WriteByte(byte(unit))
		default:
			fmt.Fprintf(&readable, "~%04X", unit)
		}
		separators = false
	}
	slug := strings.TrimLeft(readable.String(), "-")
	if slug == "" {
		slug = "root"
	}
	if len(slug) > 251 {
		slug = slug[:251]
	}
	return "--" + slug + "--"
}

var dshLogName = regexp.MustCompile(`^session(?:\.v([1-9][0-9]*))?\.jsonl(?:\.zstd)?$`)

// dshLog is the log to read in a session's directory: the highest format
// generation there, which is the one dsh reads.
func dshLog(dir string) string {
	entries, err := os.ReadDir(dir)
	if err != nil {
		return ""
	}
	best, bestVersion, bestTime := "", -1, int64(0)
	for _, entry := range entries {
		match := dshLogName.FindStringSubmatch(entry.Name())
		info, err := entry.Info()
		if match == nil || err != nil || !info.Mode().IsRegular() {
			continue
		}
		version, _ := strconv.Atoi(match[1])
		if at := info.ModTime().UnixNano(); version > bestVersion || version == bestVersion && at > bestTime {
			best, bestVersion, bestTime = filepath.Join(dir, entry.Name()), version, at
		}
	}
	return best
}

// dshSessions lists the logs of the sessions dsh ran in cwd. A sub-agent's
// session is one of them only when asked for. dsh records the directory it
// was started in with links resolved, so cwd is looked up that way too.
func dshSessions(root, cwd string, subAgents bool) []string {
	dirs := []string{filepath.Clean(cwd)}
	if real, err := filepath.EvalSymlinks(cwd); err == nil && real != dirs[0] {
		dirs = append(dirs, real)
	}
	var files []string
	for _, dir := range dirs {
		project := filepath.Join(root, dshProjectKey(dir))
		entries, _ := os.ReadDir(project)
		for _, entry := range entries {
			file := dshLog(filepath.Join(project, entry.Name()))
			if file == "" {
				continue
			}
			header := firstJSONLine(file)
			if filepath.Clean(strOr(header["cwd"], "")) != dir || !subAgents && strOr(header["origin"], "") == "subagent" {
				continue
			}
			files = append(files, file)
		}
	}
	return files
}

type dshChild struct{ id, label, file string }

// dshChildren lists the sub-agents a session started. dsh notes each one's
// session id and label in the parent's log; the sub-agent's own log is in the
// same store, in the directory named for that id, under the project it
// worked in.
func dshChildren(transcript string) []dshChild {
	lines, err := readLines(transcript)
	if err != nil {
		return nil
	}
	project := filepath.Dir(filepath.Dir(transcript))
	root := filepath.Dir(project)
	projects := []string{project}
	if entries, err := os.ReadDir(root); err == nil {
		for _, entry := range entries {
			if dir := filepath.Join(root, entry.Name()); dir != project {
				projects = append(projects, dir)
			}
		}
	}
	var children []dshChild
	seen := map[string]bool{}
	for _, line := range lines {
		if !strings.Contains(line, `"subagent/catalog"`) && !strings.Contains(line, `"session/end-seed"`) {
			continue
		}
		var event map[string]any
		if json.Unmarshal([]byte(line), &event) != nil {
			continue
		}
		data := m(event["data"])
		// A forked session begins with a copy of its parent's history, the
		// parent's sub-agents included. Its own come after the copy ends.
		if strOr(event["type"], "") == "session/end-seed" && data["inherited"] == true {
			children, seen = nil, map[string]bool{}
		}
		if strOr(event["type"], "") != "subagent/catalog" {
			continue
		}
		id := strOr(data["childId"], "")
		if id == "" || seen[id] {
			continue
		}
		seen[id] = true
		for _, dir := range projects {
			if file := dshLog(filepath.Join(dir, dshSegment(id))); file != "" {
				children = append(children, dshChild{id: id, label: strOr(data["label"], ""), file: file})
				break
			}
		}
	}
	return children
}

// dshAgents reads the sessions of a session's sub-agents, and of theirs.
func dshAgents(transcript string, depth int) []Agent {
	var agents []Agent
	for _, child := range dshChildren(transcript) {
		lines, err := readLines(child.file)
		if err != nil {
			continue
		}
		agent := Agent{ID: child.id, Name: child.label, Input: Input{Lines: lines, Fallback: child.id, Blobs: dshAttachments(child.file)}}
		if depth < 8 {
			agent.Input.Agents = dshAgents(child.file, depth+1)
		}
		agents = append(agents, agent)
	}
	return agents
}

// dshAttachments is where dsh keeps the images a log refers to: its
// attachment store, beside the sessions directory.
func dshAttachments(transcript string) string {
	home := filepath.Dir(filepath.Dir(filepath.Dir(filepath.Dir(transcript))))
	return filepath.Join(home, "attachments", "v1", "objects")
}

func latestDsh(cwd string) (*Found, bool) {
	return newestOf(dshSessions(dshSessionsDir(), cwd, false), "dsh")
}

func anywhereDsh() (*Elsewhere, bool) {
	root := dshSessionsDir()
	projects, _ := os.ReadDir(root)
	best, bestTime := "", int64(0)
	for _, project := range projects {
		sessions, _ := os.ReadDir(filepath.Join(root, project.Name()))
		for _, session := range sessions {
			file := dshLog(filepath.Join(root, project.Name(), session.Name()))
			if file == "" {
				continue
			}
			if info, err := os.Stat(file); err == nil && info.ModTime().UnixNano() > bestTime && strOr(firstJSONLine(file)["origin"], "") != "subagent" {
				best, bestTime = file, info.ModTime().UnixNano()
			}
		}
	}
	if best == "" {
		return nil, false
	}
	name, prompt, _ := dshTitles(best)
	if name == "" {
		name = prompt
	}
	return &Elsewhere{Harness: "dsh", Dir: strOr(firstJSONLine(best)["cwd"], ""), File: best, Title: name, Recency: bestTime}, true
}

// dshTitles reads what a listing shows of a session: the title dsh gave it,
// the first thing the person typed, and when it began. dsh starts every
// session with a title cut from the prompt; that one is not a title.
func dshTitles(file string) (name, prompt string, started time.Time) {
	lines, err := readLines(file)
	if err != nil || len(lines) == 0 {
		return "", "", time.Time{}
	}
	var header map[string]any
	if json.Unmarshal([]byte(lines[0]), &header) == nil {
		if ms, ok := header["createdAt"].(float64); ok && ms > 0 {
			started = time.UnixMilli(int64(ms))
		}
	}
	for _, line := range lines[1:] {
		titled, typed := strings.Contains(line, `"session/title"`), prompt == "" && strings.Contains(line, `"user/message"`)
		if !titled && !typed {
			continue
		}
		var event map[string]any
		if json.Unmarshal([]byte(line), &event) != nil {
			continue
		}
		data := m(event["data"])
		switch strOr(event["type"], "") {
		case "session/title":
			if t := strings.TrimSpace(strOr(data["title"], "")); t != "" && strOr(m(data["source"])["kind"], "") != "fallback" {
				name = t
			}
		case "user/message":
			if prompt == "" && strOr(m(data["source"])["kind"], "") == "user" {
				for _, part := range arr(data["content"]) {
					if text := strings.TrimSpace(strOr(m(part)["text"], "")); text != "" {
						prompt = text
						break
					}
				}
			}
		}
	}
	return name, prompt, started
}

// ActiveSession is the session whose agent is running this process, when its
// harness says. dsh gives every command of its shell tool the id of the
// session that ran it in DSH_SESSION_ID, beside DSH_SHELL and its own home.
func ActiveSession() (harness, id string, ok bool) {
	if id := os.Getenv("DSH_SESSION_ID"); id != "" && os.Getenv("DSH_SHELL") != "" {
		return "dsh", id, true
	}
	return "", "", false
}

// dshByID finds a session's log by its id, whichever project it is filed
// under.
func dshByID(root, id string) (string, bool) {
	projects, _ := os.ReadDir(root)
	for _, project := range projects {
		if file := dshLog(filepath.Join(root, project.Name(), dshSegment(id))); file != "" && strOr(firstJSONLine(file)["id"], "") == id {
			return file, true
		}
	}
	return "", false
}

// DshTranscript finds the log of the dsh session with this id among the
// sessions of cwd, a sub-agent's included.
func DshTranscript(cwd, id string) (string, bool) {
	for _, file := range dshSessions(dshSessionsDir(), cwd, true) {
		if strOr(firstJSONLine(file)["id"], "") == id {
			return file, true
		}
	}
	return "", false
}

/* ---------------------------------------------------------------- aider
   Aider has no session store. It appends every run in a project to
   .aider.chat.history.md in the directory it was started in, the git root
   when there is one, and what was typed at its prompt to
   .aider.input.history beside it. */

const aiderChatName = ".aider.chat.history.md"

// aiderChat says whether a file is an aider chat history: it is named like
// one, or its first line opens a run.
func aiderChat(file string) bool {
	if strings.HasSuffix(filepath.Base(file), "chat.history.md") {
		return true
	}
	f, err := os.Open(file)
	if err != nil {
		return false
	}
	defer f.Close()
	head := make([]byte, 4096)
	n, _ := io.ReadFull(f, head)
	for _, line := range strings.Split(string(head[:n]), "\n") {
		if strings.TrimSpace(line) != "" {
			return aiderStart.MatchString(strings.TrimSuffix(line, "\r"))
		}
	}
	return false
}

type aiderRun struct {
	id      string // the time the run started, as aider wrote it
	started time.Time
	lines   []string // from the run's opening line to the next run's
}

// aiderRuns splits a chat history into its runs, oldest first. Lines ahead
// of the first opening line are the end of a run whose start is gone, and
// are left out.
func aiderRuns(file string) ([]aiderRun, error) {
	raw, err := os.ReadFile(file)
	if err != nil {
		return nil, err
	}
	var runs []aiderRun
	for _, line := range strings.Split(string(raw), "\n") {
		if match := aiderStart.FindStringSubmatch(strings.TrimSuffix(line, "\r")); match != nil {
			if started, ok := aiderTime(aiderStamp, match[1]); ok {
				runs = append(runs, aiderRun{id: aiderID(match[1]), started: started})
			}
		}
		if len(runs) > 0 {
			runs[len(runs)-1].lines = append(runs[len(runs)-1].lines, line)
		}
	}
	return runs, nil
}

// aiderPrompt is the first thing typed in a run that was not a command to
// aider itself.
func aiderPrompt(run aiderRun) string {
	for _, block := range aiderBlocks(run.lines[1:]) {
		if text := aiderText(block.lines); block.kind == 'u' && text != "" && !strings.HasPrefix(text, "/") {
			return text
		}
	}
	return ""
}

// aiderInputs reads the input history aider keeps beside a chat history:
// each entry typed at its prompt, under a line with the time.
func aiderInputs(chat string) []map[string]any {
	name := strings.TrimSuffix(filepath.Base(chat), "chat.history.md") + "input.history"
	raw, err := os.ReadFile(filepath.Join(filepath.Dir(chat), name))
	if err != nil {
		return nil
	}
	var entries []map[string]any
	var text []string
	end := func() {
		if len(entries) > 0 {
			entries[len(entries)-1]["text"] = strings.Join(text, "\n")
		}
		text = nil
	}
	for _, line := range strings.Split(string(raw), "\n") {
		line = strings.TrimSuffix(line, "\r")
		if at, timed := strings.CutPrefix(line, "# "); timed {
			end()
			entries = append(entries, map[string]any{"at": strings.TrimSpace(at)})
		} else if typed, ok := strings.CutPrefix(line, "+"); ok {
			text = append(text, typed)
		}
	}
	end()
	return entries
}

// aiderInput is the importer's input for one run of a chat history: the one
// that started at the time id names, or the last. It carries the input
// history's entries from that run, and the project directory when the file
// is where aider itself keeps it.
func aiderInput(file, id string) (Input, error) {
	runs, err := aiderRuns(file)
	if err != nil {
		return Input{}, err
	}
	pick := len(runs) - 1
	if id != "" {
		pick = -1
		for i := range runs {
			if runs[i].id == id {
				pick = i
			}
		}
	}
	if pick < 0 {
		if id != "" {
			return Input{}, fmt.Errorf("no aider run started at %s in %s", id, file)
		}
		return Input{}, fmt.Errorf("%s holds no aider run", file)
	}
	run := runs[pick]
	inputs := []any{}
	for _, entry := range aiderInputs(file) {
		at, ok := aiderTime(aiderInputStamp, strOr(entry["at"], ""))
		if !ok || at.Before(run.started) || pick+1 < len(runs) && !at.Before(runs[pick+1].started) {
			continue
		}
		inputs = append(inputs, entry)
	}
	session := map[string]any{"id": run.id, "inputs": inputs}
	if abs, err := filepath.Abs(file); err == nil && filepath.Base(abs) == aiderChatName {
		session["cwd"] = filepath.Dir(abs)
	}
	return Input{Lines: run.lines, Fallback: run.id, Session: session}, nil
}

func latestAider(cwd string) (*Found, bool) {
	file := filepath.Join(cwd, aiderChatName)
	info, err := os.Stat(file)
	if runs, _ := aiderRuns(file); err != nil || len(runs) == 0 {
		return nil, false
	}
	found := fileInput(file, "aider", info.ModTime().UnixNano())
	return &found, true
}

// AiderRun finds the run of cwd's chat history that started at the time id
// names.
func AiderRun(cwd, id string) (*Found, bool) {
	file := filepath.Join(cwd, aiderChatName)
	runs, _ := aiderRuns(file)
	for _, run := range runs {
		if run.id == id {
			return &Found{Harness: "aider", Recency: run.started.UnixNano(), load: func() (Input, error) { return aiderInput(file, id) }}, true
		}
	}
	return nil, false
}
