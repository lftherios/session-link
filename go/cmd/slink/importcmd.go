package main

import (
	"bufio"
	"encoding/json"
	"errors"
	"flag"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/importers"
	"github.com/lftherios/session-link/internal/spool"
	"golang.org/x/term"
)

// experimental harnesses print a best-effort note (parity with the JS set).
var experimental = map[string]bool{"hermes": true}

type importFlags struct {
	from    string
	session string
	arg     string
}

// latestWithWalkUp looks for a harness session for cwd, walking up parent
// directories to the home/root boundary — running from a subdirectory of
// the project is the common case, and the agent histories key on the
// project root. Returns the Found and the directory that matched.
func latestWithWalkUp(harness, cwd string) (*importers.Found, string, bool) {
	home, _ := os.UserHomeDir()
	dir := cwd
	for {
		if f, ok := importers.Latest(harness, dir); ok {
			return f, dir, true
		}
		parent := filepath.Dir(dir)
		if parent == dir || dir == home {
			return nil, "", false
		}
		dir = parent
	}
}

// resolveImport finds the session to import, returning the harness + Input
// + the Found (nil when loading from an explicit file) + matched dir.
func resolveImport(f importFlags) (string, importers.Input, *importers.Found, string, error) {
	target := f.arg
	if target == "" {
		target = f.session
	}
	dbHarness := f.from == "opencode" || f.from == "hermes"

	if target != "" {
		// A db id: --from opencode|hermes --session <id>.
		if dbHarness {
			found, ok := importers.LatestByID(f.from, target)
			if !ok {
				return "", importers.Input{}, nil, "", fmt.Errorf("%s session %q not found", f.from, target)
			}
			in, err := found.Load()
			if err != nil && errors.Is(err, fs.ErrNotExist) {
				// The agent's database isn't there — say that, not a raw stat error.
				return "", importers.Input{}, nil, "", fmt.Errorf("✗ no %s database found on this machine\n\n  is %s installed here? its sessions can only be imported where it runs", f.from, f.from)
			}
			return f.from, in, found, "", err
		}
		// Not a file on disk? A pasted session id (Claude Code names its
		// transcripts <uuid>.jsonl) is worth trying before giving up.
		if !fileExists(target) {
			if p, ok := transcriptByID(target); ok {
				target = p
			} else {
				return "", importers.Input{}, nil, "", fmt.Errorf("cannot read %s — pass a transcript path, or a session id from your agent's history", target)
			}
		}
		in, harness, err := importers.LoadFile(target)
		if f.from != "" {
			harness = f.from
		}
		if harness == "" {
			return "", in, nil, "", fmt.Errorf("could not detect the agent for %s — pass --from", target)
		}
		return harness, in, nil, "", err
	}

	cwd, _ := os.Getwd()
	found, matched, ok := latestWithWalkUp(f.from, cwd)
	if !ok {
		return "", importers.Input{}, nil, "", fmt.Errorf("%s", noSessionHint(f.from, cwd))
	}
	in, err := found.Load()
	return found.Harness, in, found, matched, err
}

// noSessionHint renders import's empty state. "Nothing to import here" is
// three different situations, and the binary can tell which one the user
// is in — so it says so, with one next step each, instead of asking the
// user a question it can answer itself.
func noSessionHint(from, cwd string) string {
	record := "  or record fresh here:  slink record -- <your agent command>"
	if e, ok := importers.NewestAnywhere(from); ok {
		// Situation 1: history exists — just not for this project. Point at
		// the newest session elsewhere, with a copy-pasteable way to it.
		head := fmt.Sprintf("✗ no agent session for this project (%s or its parents)", displayPath(cwd))
		if from != "" {
			head = fmt.Sprintf("✗ no %s session for this project (%s or its parents)", from, displayPath(cwd))
		}
		desc := e.Harness
		if e.Title != "" {
			desc = fmt.Sprintf("%q · %s", clip(e.Title, 48), e.Harness)
		}
		when := ""
		if e.Recency > 0 {
			when = " · " + ago(time.Unix(0, e.Recency).UTC().Format(time.RFC3339))
		}
		var where, next string
		switch {
		case e.Dir != "":
			where = " · " + displayPath(e.Dir)
			fromFlag := ""
			if from != "" {
				fromFlag = " --from " + from
			}
			next = fmt.Sprintf("    → cd %s && slink import%s", shellPath(e.Dir), fromFlag)
		case e.File != "":
			where = ""
			next = fmt.Sprintf("    → slink import %s", shellPath(e.File))
		}
		return fmt.Sprintf("%s\n\n  your newest session elsewhere:\n    %s%s%s\n%s\n\n%s",
			head, desc, where, when, next, record)
	}
	// Situation 2: no history for this agent — or any agent — at all.
	if from != "" {
		return fmt.Sprintf("✗ no %s history found on this machine\n  (known agents: %s)\n\n  record a session instead:  slink record -- <your agent command>",
			from, strings.Join(knownHarnesses(), ", "))
	}
	return fmt.Sprintf("✗ no coding-agent history found on this machine\n  (looked for: %s)\n\n  record a session instead:  slink record -- <your agent command>",
		strings.Join(knownHarnesses(), ", "))
}

// displayPath abbreviates the home directory to ~ for reading.
func displayPath(p string) string {
	if h, err := os.UserHomeDir(); err == nil && strings.HasPrefix(p, h+string(os.PathSeparator)) {
		return "~" + p[len(h):]
	}
	return p
}

// shellPath renders a path for copy-pasting into a shell: ~-abbreviated
// when clean, double-quoted (full path, so ~ expansion isn't needed)
// when it contains spaces or quotes.
func shellPath(p string) string {
	if strings.ContainsAny(p, " '\"\\$`") {
		return `"` + strings.ReplaceAll(p, `"`, `\"`) + `"`
	}
	return displayPath(p)
}

// transcriptByID resolves a bare Claude Code session id to its transcript
// file for this project (walking up like the auto-detect does).
func transcriptByID(id string) (string, bool) {
	if strings.ContainsAny(id, "/\\.") {
		return "", false
	}
	cwd, _ := os.Getwd()
	home, _ := os.UserHomeDir()
	dir := cwd
	for {
		p := filepath.Join(importers.ClaudeProjectDir(dir), id+".jsonl")
		if fileExists(p) {
			return p, true
		}
		parent := filepath.Dir(dir)
		if parent == dir || dir == home {
			return "", false
		}
		dir = parent
	}
}

type imported struct {
	file    string
	name    string
	harness string
	spans   int
	created string
}

// importToFile reconstructs a session/v0 capture and writes it to
// ~/.slink/runs.
func importToFile(f importFlags) imported {
	if f.from != "" && importers.Registry[f.from] == nil {
		die(fmt.Sprintf("unknown agent %q — known: %s", f.from, strings.Join(knownHarnesses(), ", ")))
	}
	harness, in, found, matched, err := resolveImport(f)
	if err != nil {
		die(err.Error())
	}
	build := importers.Registry[harness]
	if build == nil {
		die("no importer for " + harness)
	}
	if experimental[harness] {
		fmt.Fprintf(os.Stderr, "note: %s import is experimental — its mapping is inferred, not dogfooded\n", harness)
	}
	run, err := build(in)
	if err != nil {
		die(fmt.Sprintf("import failed: %v", err))
	}
	spans, _ := run["spans"].([]any)
	if len(spans) == 0 {
		// An empty build must fail loudly, not write a `null`/hollow capture
		// that list skips and push rejects.
		if src := firstNonEmptyStr(f.arg, f.session); src != "" {
			die(fmt.Sprintf("nothing to import from %s — the transcript has no messages", src))
		}
		die(fmt.Sprintf("nothing to import — the newest %s session has no messages", harness))
	}
	b, err := json.Marshal(run)
	if err != nil {
		die(fmt.Sprintf("serialize: %v", err))
	}
	name, _ := run["name"].(string)
	created, _ := run["created_at"].(string)
	// Re-importing the same unchanged session is common (share retries,
	// running share twice) — reuse the byte-identical capture instead of
	// littering the store with duplicates.
	if prev := findIdenticalCapture(b); prev != "" {
		fmt.Fprintf(os.Stderr, "✓ already imported, unchanged — reusing %s\n", captureID(prev))
		fmt.Fprintf(os.Stderr, "    stored:  %s\n", displayPath(prev))
		spans, _ := run["spans"].([]any)
		return imported{file: prev, name: name, harness: harness, spans: len(spans), created: created}
	}
	file := spool.NewCapturePath(cli.CaptureDir(), time.Now())
	os.MkdirAll(filepath.Dir(file), 0o755)
	if err := os.WriteFile(file, b, 0o644); err != nil {
		die(fmt.Sprintf("write: %v", err))
	}

	age := ""
	if found != nil && found.Recency > 0 {
		age = " · last active " + ago(time.Unix(0, found.Recency).UTC().Format(time.RFC3339))
	} else if created != "" {
		age = " · started " + ago(created)
	}
	from := ""
	if cwd, _ := os.Getwd(); matched != "" && matched != cwd {
		from = " · " + displayPath(matched)
	}
	// One meaning per line — what happened, what it is, what it holds,
	// where it lives — instead of a single wrap-where-it-may banner.
	fmt.Fprintf(os.Stderr, "✓ imported from %s%s (reconstructed)\n", harness, from)
	fmt.Fprintf(os.Stderr, "    %q\n", clip(name, 76))
	fmt.Fprintf(os.Stderr, "    %s%s\n", plural(len(spans), "span"), age)
	fmt.Fprintf(os.Stderr, "    stored:  %s\n", displayPath(file))
	return imported{file: file, name: name, harness: harness, spans: len(spans), created: created}
}

// findIdenticalCapture scans recent captures for byte-identical content
// (bounded — imports land newest-first, so a match is near the top).
func findIdenticalCapture(b []byte) string {
	captures := cli.ListCaptures(cli.CaptureDir())
	for i, c := range captures {
		if i >= 50 {
			break
		}
		if fi, err := os.Stat(c.File); err != nil || fi.Size() != int64(len(b)) {
			continue
		}
		if prev, err := os.ReadFile(c.File); err == nil && string(prev) == string(b) {
			return c.File
		}
	}
	return ""
}

func knownHarnesses() []string {
	var names []string
	for k := range importers.Registry {
		names = append(names, k)
	}
	sort.Strings(names)
	return names
}

func firstNonEmptyStr(vals ...string) string {
	for _, v := range vals {
		if v != "" {
			return v
		}
	}
	return ""
}

func runImport(args []string) {
	fs := flag.NewFlagSet("import", flag.ExitOnError)
	from := fs.String("from", "", "agent: claude-code|codex|pi|opencode|hermes")
	session := fs.String("session", "", "session id, or a transcript path")
	setUsage(fs, "slink import [flags] [transcript]",
		"Convert your coding agent's newest session for this project into a\n  local session — no re-run needed. Marked fidelity: reconstructed.",
		"slink import --from claude-code")
	parseReordered(fs, args)
	imp := importToFile(importFlags{from: *from, session: *session, arg: fs.Arg(0)})
	id := captureID(imp.file)
	fmt.Fprintf(os.Stderr, "\n  review:  slink view %s\n  publish: slink share %s\n", id, id)
	// No bare path on stdout — the stored: line covers humans, and scripts
	// get structured paths from `slink list --json`.
}

// runShare is the golden path: find the session you mean — your newest
// local one or your agent's newest, whichever is fresher — review the
// selection, confirm once, sign in only at the moment it's needed,
// publish, and print the way back.
func runShare(args []string) {
	fs := flag.NewFlagSet("share", flag.ExitOnError)
	from := fs.String("from", "", "agent: claude-code|codex|pi|opencode|hermes")
	session := fs.String("session", "", "session id, or a transcript path")
	pick := fs.Bool("pick", false, "choose from recent sessions")
	yes := fs.Bool("yes", false, "publish without confirmation")
	server := fs.String("server", "", "override the publish target")
	key := fs.String("key", "", "override the API key")
	setUsage(fs, "slink share [flags] [session-id | transcript]",
		"Publish the session you mean, in one step: your newest local session\n  or your agent's newest for this project — whichever is fresher.\n  Signs you in along the way if needed.",
		"slink share")
	parseReordered(fs, args)

	var file string
	switch {
	case fs.Arg(0) != "" || *session != "":
		file = shareTarget(fs.Arg(0), *session, *from)
	case *pick:
		file = sharePick(*from)
	default:
		file = shareNewest(*from)
	}

	target, apiKey := cli.ResolveTarget(*server, *key)
	// Just-in-time sign-in: the whole point of share is zero setup before
	// value, so an account is asked for at the moment it's needed — never
	// earlier. Scripts (non-TTY) get the crisp error instead.
	if apiKey == "" && target == "https://session.link" {
		if !term.IsTerminal(int(os.Stdin.Fd())) {
			die("not signed in — run: slink login  (publishing needs an account; capture doesn't)")
		}
		fmt.Fprintln(os.Stderr, "Sign-in required — publishing needs a free account (capture doesn't).")
		r, err := cli.BrowserLogin(target, func(line string) { fmt.Fprintln(os.Stderr, line) })
		if err != nil {
			die(err.Error())
		}
		fmt.Fprintf(os.Stderr, "✓ signed in as @%s\n", r.Login)
		_, apiKey = cli.ResolveTarget(*server, *key)
	}
	publishFile(file, target, apiKey, *yes)
}

// shareNewest picks between the newest local session and the agent's
// newest for this project — whichever is fresher — and says which it chose.
// An explicit --from always means the agent: falling back to a local
// capture behind that flag would publish something the user didn't ask for.
func shareNewest(from string) string {
	cwd, _ := os.Getwd()
	found, matched, haveAgent := latestWithWalkUp(from, cwd)
	if from != "" {
		if !haveAgent {
			die(noSessionHint(from, cwd))
		}
		fmt.Fprintf(os.Stderr, "Found the latest %s session in this project:\n", found.Harness)
		return importToFile(importFlags{from: from}).file
	}
	captures := cli.ListCaptures(cli.CaptureDir())

	// Freshness is last activity on both sides: the agent side is the
	// transcript's mtime, so the local side must be the capture file's
	// mtime too — created_at is the recording's START and would lose to
	// any transcript touched after it (including the transcript of the
	// very session just recorded).
	var localAt time.Time
	if len(captures) > 0 {
		localAt, _ = time.Parse(time.RFC3339, captures[0].CreatedAt)
		if fi, err := os.Stat(captures[0].File); err == nil && fi.ModTime().After(localAt) {
			localAt = fi.ModTime()
		}
	}
	agentNewer := haveAgent && found.Recency > localAt.UnixNano()

	if len(captures) == 0 && !haveAgent {
		die(emptyState)
	}
	if agentNewer || len(captures) == 0 {
		label := found.Harness
		if matched != "" && matched != cwd {
			label += " (from " + matched + ")"
		}
		fmt.Fprintf(os.Stderr, "Found the latest %s session in this project:\n", label)
		return importToFile(importFlags{from: found.Harness}).file
	}
	c := captures[0]
	fmt.Fprintln(os.Stderr, "Sharing your newest local session:")
	printSessionRow(os.Stderr, c, "")
	return c.File
}

// shareTarget resolves an explicit share argument: a local session id, a
// capture .json (published as-is), a transcript file, or a db id — the
// last two go through import first.
func shareTarget(arg, session, from string) string {
	ref := firstNonEmptyStr(arg, session)
	if f, err := resolveCapture(ref); err == nil && isSessionDoc(f) && session == "" && from == "" {
		return f
	}
	return importToFile(importFlags{from: from, session: session, arg: arg}).file
}

// isSessionDoc cheaply checks whether a file is a session/v0 document (a
// local capture) as opposed to an agent transcript that needs importing.
func isSessionDoc(file string) bool {
	f, err := os.Open(file)
	if err != nil {
		return false
	}
	defer f.Close()
	head := make([]byte, 4096)
	n, _ := f.Read(head)
	return strings.Contains(string(head[:n]), `"session/v0"`) || strings.Contains(string(head[:n]), `"run/v0"`)
}

// sharePick is the merged picker: local sessions plus the agent's newest,
// one list, one choice.
func sharePick(from string) string {
	captures := cli.ListCaptures(cli.CaptureDir())
	cwd, _ := os.Getwd()
	found, _, haveAgent := latestWithWalkUp(from, cwd)
	if len(captures) == 0 && !haveAgent {
		die(emptyState)
	}
	if !term.IsTerminal(int(os.Stdin.Fd())) {
		die("--pick needs a TTY")
	}
	if haveAgent {
		fmt.Fprintf(os.Stderr, "  %-7s %-9s your %s agent's newest session for this project (import + publish)\n",
			"agent", ago(time.Unix(0, found.Recency).UTC().Format(time.RFC3339)), found.Harness)
	}
	show := len(captures)
	if show > 10 {
		show = 10
	}
	for _, c := range captures[:show] {
		printSessionRow(os.Stderr, c, "")
	}
	if len(captures) > show {
		fmt.Fprintf(os.Stderr, "  … and %d more — any id from `slink list` works too\n", len(captures)-show)
	}
	def := "agent"
	if !haveAgent {
		def = captureID(captures[0].File)
	}
	fmt.Fprintf(os.Stderr, "share which? [%s] ", def)
	line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
	ref := strings.TrimSpace(line)
	if ref == "" {
		ref = def
	}
	if ref == "agent" {
		if !haveAgent {
			die("no agent session found for this project")
		}
		return importToFile(importFlags{from: found.Harness}).file
	}
	file, err := resolveCapture(ref)
	if err != nil {
		die(err.Error())
	}
	return file
}

func fileExists(p string) bool {
	st, err := os.Stat(p)
	return err == nil && !st.IsDir()
}
