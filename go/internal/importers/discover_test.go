package importers

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// isolateStores points every harness's store at an empty home, whatever the
// environment the tests run in has configured.
func isolateStores(t *testing.T) string {
	t.Helper()
	home := t.TempDir()
	t.Setenv("HOME", home)
	for _, name := range []string{"CLAUDE_CONFIG_DIR", "PI_CODING_AGENT_DIR", "PI_CODING_AGENT_SESSION_DIR", "PI_CONFIG_DIR", "OMP_PROFILE", "PI_PROFILE", "CODEX_HOME", "XDG_DATA_HOME", "OPENCODE_DB", "HERMES_HOME", "HERMES_STATE_DB", "DSH_HOME"} {
		t.Setenv(name, "")
	}
	return home
}

func TestNotFoundIDErrors(t *testing.T) {
	// No DB present → openDB fails; a bogus id must surface an error, never
	// a silent empty Input (which built a degenerate capture).
	t.Setenv("HERMES_STATE_DB", filepath.Join(t.TempDir(), "nope.db"))
	if _, err := loadHermes("bogus"); err == nil {
		t.Fatal("loadHermes on a missing DB must error")
	}
}

func TestSniffWidenedForCodex(t *testing.T) {
	// A codex rollout whose head lines are response_items (late/stripped
	// session_meta) must still sniff as codex, like JS looksLikeCodex.
	lines := []string{
		`{"type":"response_item","payload":{"type":"message","role":"user","content":[]}}`,
		`{"type":"turn_context","payload":{"model":"gpt-5"}}`,
	}
	if got := sniff(lines); got != "codex" {
		t.Fatalf("sniff widened predicate: got %q want codex", got)
	}
}

func TestAutoDetectPicksMostRecent(t *testing.T) {
	// Two harnesses' sessions in one cwd: the NEWEST wins, not a fixed order.
	isolateStores(t)
	cwd := "/work/proj"
	// An older claude-code session...
	ccDir := filepath.Join(claudeProjectsDir(), claudeSlug(cwd))
	os.MkdirAll(ccDir, 0o755)
	ccFile := filepath.Join(ccDir, "old.jsonl")
	os.WriteFile(ccFile, []byte(`{"type":"user","message":{"role":"user","content":"x"}}`+"\n"), 0o644)
	old := timeAgo(2)
	os.Chtimes(ccFile, old, old)
	// ...and a newer pi session.
	piDir := piProjectDir(piSessionsDir(), cwd)
	os.MkdirAll(piDir, 0o755)
	piFile := filepath.Join(piDir, "new.jsonl")
	os.WriteFile(piFile, []byte(`{"type":"session"}`+"\n"), 0o644)

	found, ok := Latest("", cwd)
	if !ok {
		t.Fatal("expected a session")
	}
	if found.Harness != "pi" {
		t.Fatalf("auto-detect must pick the newest across harnesses: got %s, want pi", found.Harness)
	}
}

func TestNewestAnywhere(t *testing.T) {
	home := isolateStores(t)
	t.Setenv("CODEX_HOME", filepath.Join(home, "no-codex"))
	t.Setenv("XDG_DATA_HOME", filepath.Join(home, "no-xdg"))
	t.Setenv("HERMES_STATE_DB", filepath.Join(home, "no.db"))

	if _, ok := NewestAnywhere(""); ok {
		t.Fatal("empty machine reported a session")
	}

	older := filepath.Join(home, ".claude", "projects", "-proj-a")
	newer := filepath.Join(home, ".claude", "projects", "-proj-b")
	os.MkdirAll(older, 0o755)
	os.MkdirAll(newer, 0o755)
	line := `{"type":"user","cwd":"/proj/b","message":{"role":"user","content":"Ship the empty-state UX for import"}}` + "\n"
	os.WriteFile(filepath.Join(older, "old.jsonl"), []byte(line), 0o644)
	os.WriteFile(filepath.Join(newer, "new.jsonl"), []byte(line), 0o644)
	past := time.Now().Add(-2 * time.Hour)
	os.Chtimes(filepath.Join(older, "old.jsonl"), past, past)

	e, ok := NewestAnywhere("")
	if !ok {
		t.Fatal("found nothing")
	}
	if e.Harness != "claude-code" || filepath.Base(e.File) != "new.jsonl" {
		t.Fatalf("picked %s %s, want claude-code new.jsonl", e.Harness, e.File)
	}
	if e.Dir != "/proj/b" {
		t.Fatalf("cwd = %q, want /proj/b (peeked from the transcript)", e.Dir)
	}
	if e.Title != "Ship the empty-state UX for import" {
		t.Fatalf("title = %q", e.Title)
	}
	if _, ok := NewestAnywhere("pi"); ok {
		t.Fatal("pi reported a session on a claude-only machine")
	}
}

func TestClaudeSlugMatchesClaudeCode(t *testing.T) {
	// Claude Code 2.1: cwd.replace(/[^a-zA-Z0-9]/g, "-"), one "-" per UTF-16
	// code unit.
	for path, want := range map[string]string{
		"/work/project":          "-work-project",
		"/work/my_project":       "-work-my-project",
		"/work/site.v2/app":      "-work-site-v2-app",
		"/work/Client Files/a@b": "-work-Client-Files-a-b",
		"/work/café":             "-work-caf-",
		"/work/🚀":                "-work---",
	} {
		if got := claudeSlug(path); got != want {
			t.Errorf("claudeSlug(%q) = %q, want %q", path, got, want)
		}
	}
}

func claudeEntry(cwd, prompt string) string {
	return `{"type":"user","cwd":"` + cwd + `","message":{"role":"user","content":"` + prompt + `"}}` + "\n"
}

func TestClaudeSessionsInPunctuatedPaths(t *testing.T) {
	home := isolateStores(t)
	projects := filepath.Join(home, ".claude", "projects")
	cwd := "/work/my_project"
	writeRecent(t, filepath.Join(projects, "-work-my-project", "aaaa.jsonl"), claudeEntry(cwd, "Tidy the importer"), time.Now())

	if found, ok := Latest("claude-code", cwd); !ok || found.Harness != "claude-code" {
		t.Fatal("the newest session of a project with an underscore in its path was not found")
	}
	if file, ok := ClaudeTranscript(cwd, "aaaa"); !ok || filepath.Base(file) != "aaaa.jsonl" {
		t.Fatalf("session id not resolved: %q %v", file, ok)
	}
	listed, err := DefaultLocations().Recent("claude-code", cwd+"/src", "", 30)
	if err != nil || len(listed) != 1 || listed[0].Dir != cwd || listed[0].Title != "Tidy the importer" {
		t.Fatalf("listed = %+v, %v", listed, err)
	}

	// CLAUDE_CONFIG_DIR moves the whole configuration directory.
	moved := filepath.Join(home, "elsewhere")
	writeRecent(t, filepath.Join(moved, "projects", "-work-my-project", "bbbb.jsonl"), claudeEntry(cwd, "Moved"), time.Now())
	t.Setenv("CLAUDE_CONFIG_DIR", moved)
	if _, ok := ClaudeTranscript(cwd, "bbbb"); !ok {
		t.Fatal("CLAUDE_CONFIG_DIR was not honoured")
	}
	if _, ok := ClaudeTranscript(cwd, "aaaa"); ok {
		t.Fatal("the default directory was read although CLAUDE_CONFIG_DIR is set")
	}
}

func TestClaudeSessionsInLongPaths(t *testing.T) {
	// Past 200 characters Claude Code cuts the name and appends a hash, so
	// two projects can share the cut name. The recorded directory tells
	// them apart, also from an ancestor that is itself past the limit.
	projects := t.TempDir()
	parent := "/work/" + strings.Repeat("deep-directory/", 15)
	one, two := parent+"one", parent+"two"
	cut := claudeSlug(one)[:claudeSlugLimit]
	if cut != claudeSlug(two)[:claudeSlugLimit] {
		t.Fatal("the fixture paths must share their cut name")
	}
	writeRecent(t, filepath.Join(projects, cut+"-1x2y3z", "first.jsonl"), claudeEntry(one, "In the first project"), time.Now())
	writeRecent(t, filepath.Join(projects, cut+"-9q8r7s", "second.jsonl"), claudeEntry(two, "In the second project"), time.Now())

	listed, err := Locations{Claude: projects}.Recent("claude-code", one, "", 30)
	if err != nil || len(listed) != 1 || listed[0].ID != "first" || listed[0].Dir != one {
		t.Fatalf("listed = %+v, %v", listed, err)
	}
	if files := claudeTranscripts(projects, two); len(files) != 1 || filepath.Base(files[0]) != "second.jsonl" {
		t.Fatalf("second project: %v", files)
	}
}

// Each harness can be told to keep its store somewhere else, and each is
// followed there by the rule that harness itself applies.
func TestStoreLocationsFollowEachHarness(t *testing.T) {
	home := isolateStores(t)
	mkdir := func(dir string) string {
		if err := os.MkdirAll(dir, 0o700); err != nil {
			t.Fatal(err)
		}
		return dir
	}
	check := func(what, got, want string) {
		t.Helper()
		if got != want {
			t.Errorf("%s: %s, want %s", what, got, want)
		}
	}

	root := filepath.Join(home, ".hermes")
	check("hermes default", hermesDBPath(), filepath.Join(root, "state.db"))
	work := mkdir(filepath.Join(root, "profiles", "work"))
	os.WriteFile(filepath.Join(root, "active_profile"), []byte("work\n"), 0o600)
	check("hermes active profile", hermesDBPath(), filepath.Join(work, "state.db"))
	os.WriteFile(filepath.Join(root, "active_profile"), []byte("gone\n"), 0o600)
	check("hermes profile that does not exist", hermesDBPath(), filepath.Join(root, "state.db"))
	os.WriteFile(filepath.Join(root, "active_profile"), []byte("default\n"), 0o600)
	check("hermes default profile", hermesDBPath(), filepath.Join(root, "state.db"))
	other := mkdir(filepath.Join(home, "data"))
	t.Setenv("HERMES_HOME", filepath.Join(other, "profiles", "coder"))
	check("HERMES_HOME naming a profile", hermesDBPath(), filepath.Join(other, "profiles", "coder", "state.db"))
	t.Setenv("HERMES_HOME", other)
	check("HERMES_HOME naming a root", hermesDBPath(), filepath.Join(other, "state.db"))
	mkdir(filepath.Join(other, "profiles", "ops"))
	os.WriteFile(filepath.Join(other, "active_profile"), []byte("ops"), 0o600)
	check("HERMES_HOME root with an active profile", hermesDBPath(), filepath.Join(other, "profiles", "ops", "state.db"))
	t.Setenv("HERMES_STATE_DB", filepath.Join(home, "explicit.db"))
	check("HERMES_STATE_DB", hermesDBPath(), filepath.Join(home, "explicit.db"))

	data := filepath.Join(home, ".local", "share", "opencode")
	check("opencode default", opencodeDBPath(), filepath.Join(data, "opencode.db"))
	t.Setenv("OPENCODE_DB", "opencode-dev.db")
	check("OPENCODE_DB file name", opencodeDBPath(), filepath.Join(data, "opencode-dev.db"))
	t.Setenv("OPENCODE_DB", filepath.Join(home, "elsewhere.db"))
	check("OPENCODE_DB path", opencodeDBPath(), filepath.Join(home, "elsewhere.db"))
	t.Setenv("OPENCODE_DB", ":memory:")
	check("OPENCODE_DB in memory", opencodeDBPath(), filepath.Join(data, "opencode.db"))

	check("pi default", piSessionsDir(), filepath.Join(home, ".pi", "agent", "sessions"))
	t.Setenv("PI_CODING_AGENT_DIR", "~/agents/pi")
	check("PI_CODING_AGENT_DIR", piSessionsDir(), filepath.Join(home, "agents", "pi", "sessions"))
	check("pi project directory", piProjectDir("/s", "/work/my project"), filepath.Join("/s", "--work-my project--"))
}

// Given a session directory, pi keeps every project's sessions in it, so the
// header of each transcript says which project it belongs to.
func TestPiSessionsInAConfiguredSessionDirectory(t *testing.T) {
	home := isolateStores(t)
	project, other := filepath.Join(home, "code", "proj"), filepath.Join(home, "code", "other")
	session := func(id, cwd string) string {
		return `{"type":"session","id":"` + id + `","cwd":"` + cwd + `"}` + "\n"
	}
	shared := filepath.Join(home, "pi-sessions")
	writeRecent(t, filepath.Join(shared, "mine.jsonl"), session("mine", project), time.Now())
	writeRecent(t, filepath.Join(shared, "theirs.jsonl"), session("theirs", other), time.Now())
	writeRecent(t, filepath.Join(piProjectDir(piSessionsDir(), project), "default.jsonl"), session("default", project), time.Now().Add(-time.Hour))

	list := func() string {
		listed, err := DefaultLocations().Recent("pi", project, "", 30)
		if err != nil {
			t.Fatal(err)
		}
		return candidateIDs(listed)
	}
	if got := list(); got != "default" {
		t.Fatalf("without a session directory: %q", got)
	}
	t.Setenv("PI_CODING_AGENT_SESSION_DIR", "~/pi-sessions")
	if got := list(); got != "mine,default" {
		t.Fatalf("with PI_CODING_AGENT_SESSION_DIR: %q", got)
	}
	if found, ok := Latest("pi", project); !ok || found.Recency == 0 {
		t.Fatal("the newest session in the session directory was not found")
	}

	// The same through settings.json: the agent directory's, then the
	// project's, whose relative path starts at the project.
	t.Setenv("PI_CODING_AGENT_SESSION_DIR", "")
	writeRecent(t, filepath.Join(home, ".pi", "agent", "settings.json"), `{"sessionDir":"`+shared+`"}`, time.Now())
	if got := list(); got != "mine,default" {
		t.Fatalf("with sessionDir in the agent settings: %q", got)
	}
	writeRecent(t, filepath.Join(project, ".pi", "settings.json"), `{"sessionDir":".sessions"}`, time.Now())
	writeRecent(t, filepath.Join(project, ".sessions", "local.jsonl"), session("local", project), time.Now().Add(time.Minute))
	if got := list(); got != "local,default" {
		t.Fatalf("with sessionDir in the project settings: %q", got)
	}
}

// omp names a project's session directory by where the project is: relative
// to home, relative to the temp directory, or by its whole path.
func TestOmpSessionsAreFoundWhereOmpKeepsThem(t *testing.T) {
	home := isolateStores(t)
	home, _ = filepath.EvalSymlinks(home)
	t.Setenv("HOME", home)
	tmp, _ := filepath.EvalSymlinks(t.TempDir())
	t.Setenv("TMPDIR", tmp)
	sessions := ompSessionsDir()
	if sessions != filepath.Join(home, ".omp", "agent", "sessions") {
		t.Fatalf("sessions directory: %s", sessions)
	}
	transcript := func(id, cwd, title string) string {
		return `{"type":"title","v":1,"title":"` + title + `","updatedAt":"2026-10-07T10:00:00.000Z","pad":"    "}` + "\n" +
			`{"type":"session","version":3,"id":"` + id + `","timestamp":"2026-10-07T10:00:00.000Z","cwd":"` + cwd + `"}` + "\n" +
			`{"type":"message","id":"m1","parentId":null,"timestamp":"2026-10-07T10:00:01.000Z","message":{"role":"user","content":[{"type":"text","text":"Find the race"}]}}` + "\n"
	}
	project := filepath.Join(home, "code", "my.app")
	scratch := filepath.Join(tmp, "build", "one")
	for _, dir := range []string{project, scratch} {
		if err := os.MkdirAll(dir, 0o700); err != nil {
			t.Fatal(err)
		}
	}
	now := time.Now()
	writeRecent(t, filepath.Join(sessions, "-code-my.app", "a.jsonl"), transcript("in-home", project, "Upload test race"), now)
	// A sub-agent's transcript sits in a directory beside its session's file.
	writeRecent(t, filepath.Join(sessions, "-code-my.app", "a", "Scout.jsonl"), transcript("sub-agent", project, ""), now.Add(time.Minute))
	// Last used before omp named directories relative to home.
	writeRecent(t, piProjectDir(sessions, project)+"/b.jsonl", transcript("older-naming", project, ""), now.Add(-time.Hour))
	writeRecent(t, filepath.Join(sessions, "-", "c.jsonl"), transcript("at-home", home, ""), now)
	writeRecent(t, filepath.Join(sessions, "-tmp-build-one", "d.jsonl"), transcript("in-tmp", scratch, ""), now)
	writeRecent(t, filepath.Join(sessions, "--work-elsewhere--", "e.jsonl"), transcript("elsewhere", "/work/elsewhere", ""), now)

	loc := DefaultLocations()
	list := func(cwd string) []Candidate {
		t.Helper()
		listed, err := loc.Recent("omp", cwd, "", 30)
		if err != nil {
			t.Fatal(err)
		}
		return listed
	}
	inProject := list(filepath.Join(project, "src"))
	if candidateIDs(inProject) != "in-home,older-naming" {
		t.Fatalf("in the project: %q", candidateIDs(inProject))
	}
	if first := inProject[0]; first.Harness != "omp" || first.Title != "Upload test race" || first.Prompt != "Find the race" || first.Dir != project {
		t.Fatalf("listed as %+v", first)
	}
	if untitled := inProject[1]; untitled.Name != "" || untitled.Title != "Find the race" {
		t.Fatalf("an untitled session falls back to its prompt: %+v", untitled)
	}
	for cwd, want := range map[string]string{home: "at-home", scratch: "in-tmp", "/work/elsewhere": "elsewhere"} {
		if got := candidateIDs(list(cwd)); got != want {
			t.Errorf("in %s: %q, want %q", cwd, got, want)
		}
	}
	if found, ok := Latest("omp", project); !ok || found.Harness != "omp" {
		t.Fatal("the newest omp session of the project was not found")
	}

	// A profile: PI_PROFILE counts only while OMP_PROFILE is not set at all.
	os.Unsetenv("OMP_PROFILE")
	t.Setenv("PI_PROFILE", "work")
	if got := ompSessionsDir(); got != filepath.Join(home, ".omp", "profiles", "work", "agent", "sessions") {
		t.Errorf("PI_PROFILE: %s", got)
	}
	t.Setenv("OMP_PROFILE", "")
	if got := ompSessionsDir(); got != sessions {
		t.Errorf("an empty OMP_PROFILE selects the default profile over PI_PROFILE: %s", got)
	}
	t.Setenv("OMP_PROFILE", "review")
	if got := ompSessionsDir(); got != filepath.Join(home, ".omp", "profiles", "review", "agent", "sessions") {
		t.Errorf("OMP_PROFILE: %s", got)
	}
	// The default profile's agent directory can be moved, and its data can
	// live under XDG_DATA_HOME once omp has moved it there.
	t.Setenv("OMP_PROFILE", "default")
	t.Setenv("PI_CODING_AGENT_DIR", filepath.Join(home, "agents", "omp"))
	if got := ompSessionsDir(); got != filepath.Join(home, "agents", "omp", "sessions") {
		t.Errorf("PI_CODING_AGENT_DIR: %s", got)
	}
	t.Setenv("PI_CODING_AGENT_DIR", "")
	xdg := filepath.Join(home, "xdg")
	os.MkdirAll(filepath.Join(xdg, "omp"), 0o700)
	t.Setenv("XDG_DATA_HOME", xdg)
	if got := ompSessionsDir(); got != filepath.Join(xdg, "omp", "sessions") {
		t.Errorf("XDG_DATA_HOME: %s", got)
	}
}

func TestSniffTellsOmpFromPi(t *testing.T) {
	header := `{"type":"session","version":3,"id":"s","cwd":"/work/project"}`
	if got := sniff([]string{`{"type":"title","v":1,"title":"","updatedAt":"2026-10-07T10:00:00.000Z","pad":"  "}`, header}); got != "omp" {
		t.Fatalf("an omp transcript sniffed as %q", got)
	}
	if got := sniff([]string{header}); got != "pi" {
		t.Fatalf("a pi transcript sniffed as %q", got)
	}
}

func TestImagePartTellsAnImageByItsBytes(t *testing.T) {
	// The first bytes of each format, base64-encoded.
	for data, want := range map[string]string{
		"iVBORw0KGgoAAAANSUhEUgAAAAYAAAAG": "image/png",
		"/9j/4AAQSkZJRgABAQAAAQABAAD/2wBD": "image/jpeg",
		"R0lGODlhAQABAIAAAAAAAP///yH5BAEA": "image/gif",
		"UklGRiQAAABXRUJQVlA4IBgAAAAwAQCd": "image/webp",
	} {
		part, ok := imagePart(data, "")
		if !ok || part["mime"] != want || part["url"] != "data:"+want+";base64,"+data {
			t.Errorf("%s: %v %v", want, part, ok)
		}
	}
	if part, ok := imagePart("AAAAAAAAAAAAAAAA", ""); ok {
		t.Errorf("bytes that are no image became %v", part)
	}
	if part, ok := imagePart("AAAAAAAAAAAAAAAA", "image/avif"); !ok || part["mime"] != "image/avif" {
		t.Errorf("the harness's own label must stand: %v %v", part, ok)
	}
	if _, ok := imageURLPart("data:application/pdf;base64,AAAA"); ok {
		t.Error("a data URL that is not an image became an image")
	}
	if part, ok := imageURLPart("https://example.test/chart.png"); !ok || part["url"] != "https://example.test/chart.png" {
		t.Errorf("a link: %v %v", part, ok)
	}
}
