package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/importers"
	"github.com/lftherios/session-link/internal/spool"
)

// experimental harnesses print a best-effort note (parity with the JS set).
var experimental = map[string]bool{"hermes": true}

type importFlags struct {
	from    string
	session string
	arg     string
}

// resolveImport finds the session to import, returning the harness + Input.
// A positional arg and --session are equivalent (JS parity: --session takes
// a file path OR a db id); a db id only makes sense with --from opencode|hermes.
func resolveImport(f importFlags) (string, importers.Input, error) {
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
				return "", importers.Input{}, fmt.Errorf("%s session %q not found", f.from, target)
			}
			in, err := found.Load()
			return f.from, in, err
		}
		// Otherwise it's a transcript file — must exist (JS dies on ENOENT
		// rather than silently auto-detecting a different session).
		if !fileExists(target) {
			return "", importers.Input{}, fmt.Errorf("cannot read %s", target)
		}
		in, harness, err := importers.LoadFile(target)
		if f.from != "" {
			harness = f.from
		}
		if harness == "" {
			return "", in, fmt.Errorf("could not detect the harness for %s — pass --from", target)
		}
		return harness, in, err
	}

	cwd, _ := os.Getwd()
	found, ok := importers.Latest(f.from, cwd)
	if !ok {
		if f.from != "" {
			return "", importers.Input{}, fmt.Errorf("no %s session found for %s\n  pass one explicitly: slink import --session <file.jsonl|id>", f.from, cwd)
		}
		return "", importers.Input{}, fmt.Errorf("no importable session found for %s — is a supported agent's history here?", cwd)
	}
	in, err := found.Load()
	return found.Harness, in, err
}

// importToFile reconstructs a session/v0 capture and writes it to
// ~/.slink/runs, returning the path.
func importToFile(f importFlags) string {
	if f.from != "" && importers.Registry[f.from] == nil {
		die(fmt.Sprintf("unknown agent %q — known: %s", f.from, strings.Join(knownHarnesses(), ", ")))
	}
	harness, in, err := resolveImport(f)
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
	file := spool.NewCapturePath(cli.CaptureDir(), time.Now())
	os.MkdirAll(filepath.Dir(file), 0o755)
	b, err := json.Marshal(run)
	if err != nil {
		die(fmt.Sprintf("serialize: %v", err))
	}
	if err := os.WriteFile(file, b, 0o644); err != nil {
		die(fmt.Sprintf("write: %v", err))
	}
	name, _ := run["name"].(string)
	fmt.Fprintf(os.Stderr, "✓ imported %q from %s · %d spans (reconstructed) → %s\n", name, harness, len(spans), file)
	return file
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
	session := fs.String("session", "", "session id (opencode/hermes)")
	setUsage(fs, "slink import [flags] [transcript]",
		"Convert your coding agent's newest session for this project into a\n  local session — no re-run needed. Marked fidelity: reconstructed.",
		"slink import --from claude-code")
	parseReordered(fs, args)
	file := importToFile(importFlags{from: *from, session: *session, arg: fs.Arg(0)})
	fmt.Println(file) // pipeable
}

// runShare imports the newest session and publishes it — one step. Import
// flags select the session; push flags (--server/--key/--yes) forward.
func runShare(args []string) {
	fs := flag.NewFlagSet("share", flag.ExitOnError)
	from := fs.String("from", "", "agent: claude-code|codex|pi|opencode|hermes")
	session := fs.String("session", "", "session id (opencode/hermes)")
	yes := fs.Bool("yes", false, "publish without confirmation")
	server := fs.String("server", "", "override the publish target")
	key := fs.String("key", "", "override the API key")
	setUsage(fs, "slink share [flags] [transcript]",
		"Import your agent's newest session and publish it — both steps in one.",
		"slink share")
	parseReordered(fs, args)

	file := importToFile(importFlags{from: *from, session: *session, arg: fs.Arg(0)})
	pushArgs := []string{}
	if *yes {
		pushArgs = append(pushArgs, "--yes")
	}
	if *server != "" {
		pushArgs = append(pushArgs, "--server", *server)
	}
	if *key != "" {
		pushArgs = append(pushArgs, "--key", *key)
	}
	runPush(append(pushArgs, file))
}

func fileExists(p string) bool {
	st, err := os.Stat(p)
	return err == nil && !st.IsDir()
}
