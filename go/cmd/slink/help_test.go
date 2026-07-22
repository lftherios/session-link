package main

import (
	"flag"
	"io"
	"os"
	"os/exec"
	"strings"
	"testing"
)

func TestSuggest(t *testing.T) {
	cases := map[string]string{
		"pussh":   "push",
		"lst":     "list",
		"impotr":  "import",
		"shre":    "share",
		"versoin": "version",
		"zzzzzz":  "",
	}
	for in, want := range cases {
		if got := suggest(in); got != want {
			t.Errorf("suggest(%q) = %q, want %q", in, got, want)
		}
	}
}

func newTestFS() (*flag.FlagSet, *bool, *string) {
	fs := flag.NewFlagSet("t", flag.ContinueOnError)
	fs.SetOutput(io.Discard)
	yes := fs.Bool("yes", false, "")
	server := fs.String("server", "", "")
	return fs, yes, server
}

func TestParseReorderedFlagsAfterPositional(t *testing.T) {
	fs, yes, server := newTestFS()
	parseReordered(fs, []string{"file.json", "--yes", "--server", "http://x"})
	if !*yes || *server != "http://x" {
		t.Fatalf("flags after positional dropped: yes=%v server=%q", *yes, *server)
	}
	if got := fs.Args(); len(got) != 1 || got[0] != "file.json" {
		t.Fatalf("positionals = %v, want [file.json]", got)
	}
}

func TestParseReorderedBoolDoesNotConsume(t *testing.T) {
	fs, yes, _ := newTestFS()
	parseReordered(fs, []string{"--yes", "file.json"})
	if !*yes {
		t.Fatal("--yes not parsed")
	}
	if got := fs.Args(); len(got) != 1 || got[0] != "file.json" {
		t.Fatalf("bool flag consumed the positional: args = %v", got)
	}
}

func TestParseReorderedDoubleDashStaysVerbatim(t *testing.T) {
	fs, yes, server := newTestFS()
	parseReordered(fs, []string{"--yes", "--", "--server", "x"})
	if !*yes {
		t.Fatal("--yes not parsed")
	}
	if *server != "" {
		t.Fatal("flag after -- was parsed; must stay positional")
	}
	if got := fs.Args(); len(got) != 2 || got[0] != "--server" || got[1] != "x" {
		t.Fatalf("post-\"--\" args = %v, want [--server x]", got)
	}
}

func TestParseReorderedEqualsForm(t *testing.T) {
	fs, _, server := newTestFS()
	parseReordered(fs, []string{"file.json", "--server=http://y"})
	if *server != "http://y" {
		t.Fatalf("--server=… after positional dropped: %q", *server)
	}
}

func TestParseReorderedDanglingValueFlagErrors(t *testing.T) {
	if os.Getenv("SLINK_TEST_DANGLING") == "1" {
		fs, _, _ := newTestFS()
		parseReordered(fs, []string{"file.json", "--server"})
		return // unreachable: parseReordered must exit(2)
	}
	cmd := exec.Command(os.Args[0], "-test.run", "TestParseReorderedDanglingValueFlagErrors")
	cmd.Env = append(os.Environ(), "SLINK_TEST_DANGLING=1")
	out, err := cmd.CombinedOutput()
	ee, ok := err.(*exec.ExitError)
	if !ok || ee.ExitCode() != 2 {
		t.Fatalf("want exit 2 for dangling value flag, got err=%v\n%s", err, out)
	}
	if !strings.Contains(string(out), "flag needs an argument: --server") {
		t.Fatalf("missing 'flag needs an argument' message:\n%s", out)
	}
}

func TestParseReorderedValueFlagBeforeDashDashErrors(t *testing.T) {
	if os.Getenv("SLINK_TEST_DD") == "1" {
		fs, _, _ := newTestFS()
		parseReordered(fs, []string{"--server", "--", "positional"})
		return
	}
	cmd := exec.Command(os.Args[0], "-test.run", "TestParseReorderedValueFlagBeforeDashDashErrors")
	cmd.Env = append(os.Environ(), "SLINK_TEST_DD=1")
	out, err := cmd.CombinedOutput()
	ee, ok := err.(*exec.ExitError)
	if !ok || ee.ExitCode() != 2 {
		t.Fatalf("want exit 2 when a value flag would swallow --, got err=%v\n%s", err, out)
	}
}

func TestCaptureID(t *testing.T) {
	cases := map[string]string{
		"/x/runs/20260722-091500-ab12cd.json": "ab12cd",
		"20260722-091500-ffff01.json":         "ffff01",
		"/x/runs/oddly-named.json":            "",
		"/x/runs/20260722-091500-ab12cd.json.spool": "",
	}
	for in, want := range cases {
		if got := captureID(in); got != want {
			t.Errorf("captureID(%q) = %q, want %q", in, got, want)
		}
	}
}

func TestPublishedID(t *testing.T) {
	if got := publishedID("https://session.link/r/9f3kx2mvq7wtd4"); got != "9f3kx2mvq7wtd4" {
		t.Errorf("publishedID = %q", got)
	}
	if got := publishedID("no-run-path"); got != "" {
		t.Errorf("publishedID on junk = %q, want empty", got)
	}
}
