package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"path/filepath"
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
func resolveImport(f importFlags) (string, importers.Input, error) {
	// An explicit .json/.jsonl file path — sniff the file-based harness.
	if f.arg != "" && fileExists(f.arg) {
		in, harness, err := importers.LoadFile(f.arg)
		if f.from != "" {
			harness = f.from
		}
		if harness == "" {
			return "", in, fmt.Errorf("could not detect the harness for %s — pass --from", f.arg)
		}
		return harness, in, err
	}
	cwd, _ := os.Getwd()
	// --session <id> for the DB-backed harnesses.
	if f.session != "" && (f.from == "opencode" || f.from == "hermes") {
		found, ok := importers.LatestByID(f.from, f.session)
		if !ok {
			return "", importers.Input{}, fmt.Errorf("%s session %q not found", f.from, f.session)
		}
		in, err := found.Load()
		return f.from, in, err
	}
	found, ok := importers.Latest(f.from, cwd)
	if !ok {
		if f.from != "" {
			return "", importers.Input{}, fmt.Errorf("no %s session found for %s", f.from, cwd)
		}
		return "", importers.Input{}, fmt.Errorf("no importable session found for %s — is a supported agent's history here?", cwd)
	}
	in, err := found.Load()
	return found.Harness, in, err
}

// importToFile reconstructs a session/v0 capture and writes it to
// ~/.slink/runs, returning the path.
func importToFile(f importFlags) string {
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
	file := spool.NewCapturePath(cli.CaptureDir(), time.Now())
	os.MkdirAll(filepath.Dir(file), 0o755)
	b, err := json.Marshal(run)
	if err != nil {
		die(fmt.Sprintf("serialize: %v", err))
	}
	if err := os.WriteFile(file, b, 0o644); err != nil {
		die(fmt.Sprintf("write: %v", err))
	}
	spans, _ := run["spans"].([]any)
	name, _ := run["name"].(string)
	fmt.Fprintf(os.Stderr, "✓ imported %q from %s · %d spans (reconstructed) → %s\n", name, harness, len(spans), file)
	return file
}

func runImport(args []string) {
	fs := flag.NewFlagSet("import", flag.ExitOnError)
	from := fs.String("from", "", "harness: claude-code|codex|pi|opencode|hermes")
	session := fs.String("session", "", "session id (opencode/hermes)")
	fs.Parse(args)
	file := importToFile(importFlags{from: *from, session: *session, arg: fs.Arg(0)})
	fmt.Println(file) // pipeable
}

// runShare imports the newest session and publishes it — one step. Import
// flags select the session; push flags (--server/--key/--yes) forward.
func runShare(args []string) {
	fs := flag.NewFlagSet("share", flag.ExitOnError)
	from := fs.String("from", "", "harness")
	session := fs.String("session", "", "session id (opencode/hermes)")
	yes := fs.Bool("yes", false, "publish without confirmation")
	server := fs.String("server", "", "override the publish target")
	key := fs.String("key", "", "override the API key")
	fs.Parse(args)

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
