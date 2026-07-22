package main

import (
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"

	"github.com/lftherios/session-link/internal/cli"
)

// Capture files are named <stamp>-<hex>.json (spool.NewCapturePath); the
// trailing hex is unique per capture and short enough to type — that is
// the session id every command accepts.
var idSuffix = regexp.MustCompile(`-([0-9a-f]{6})\.json$`)

func captureID(file string) string {
	if m := idSuffix.FindStringSubmatch(filepath.Base(file)); m != nil {
		return m[1]
	}
	return ""
}

// resolveCapture turns a user-supplied reference — a session id from
// `slink list`, or a path — into a capture file. Bare ids only match
// local sessions; anything with a path separator or .json suffix is
// treated as a path.
func resolveCapture(ref string) (string, error) {
	if strings.ContainsAny(ref, "/\\") || strings.HasSuffix(ref, ".json") {
		if fileExists(ref) {
			return ref, nil
		}
		return "", fmt.Errorf("cannot read %s", ref)
	}
	captures := cli.ListCaptures(cli.CaptureDir())
	var matches []string
	for _, c := range captures {
		if captureID(c.File) == ref {
			matches = append(matches, c.File)
		}
	}
	switch len(matches) {
	case 1:
		return matches[0], nil
	default:
		if len(matches) > 1 {
			return "", fmt.Errorf("id %s matches %d sessions — pass a full path instead:\n    %s", ref, len(matches), strings.Join(matches, "\n    "))
		}
	}
	if fileExists(ref) { // a bare filename in cwd
		return ref, nil
	}
	return "", fmt.Errorf("no session %q — ids are in the first column of `slink list`", ref)
}

// printSessionRow is the one renderer every capture listing uses — list,
// push --pick, share --pick — so the id a user sees is the id every
// command accepts.
func printSessionRow(w *os.File, c cli.Capture, extra string) {
	marker := ""
	if c.InProgress {
		marker = "  (recording)"
	}
	fmt.Fprintf(w, "  %-7s %-9s %-38s %s  %s%s%s\n",
		captureID(c.File), ago(c.CreatedAt), clip(c.Name, 38),
		plural(c.Spans, "span"), strings.Join(c.Models, ", "), marker, extra)
}
