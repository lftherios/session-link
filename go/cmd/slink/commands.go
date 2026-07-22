package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"golang.org/x/term"
)

// emptyState is the shared "no sessions yet" block — every surface that
// can hit an empty store points at the same three ways in, one per line.
const emptyState = `no sessions yet

  record:  slink record -- <your agent command>
  import:  slink import
  ambient: slink setup`

func runList(args []string) {
	fs := flag.NewFlagSet("list", flag.ExitOnError)
	limit := fs.Int("limit", 10, "rows to show")
	asJSON := fs.Bool("json", false, "emit JSON (one array; ids, paths, and metadata)")
	setUsage(fs, "slink list [flags]",
		"List local sessions, newest first. Nothing here has been published.\n  The first column is the session id — view, push, and share accept it.\n  (delete takes a published URL, not a local id.)",
		"slink list --limit 25")
	parseReordered(fs, args)
	if *limit < 1 {
		*limit = 1
	}
	captures := cli.ListCaptures(cli.CaptureDir())
	if *asJSON {
		type row struct {
			ID         string   `json:"id"`
			Name       string   `json:"name"`
			CreatedAt  string   `json:"created_at"`
			Spans      int      `json:"spans"`
			Models     []string `json:"models"`
			File       string   `json:"file"`
			InProgress bool     `json:"in_progress"`
		}
		rows := make([]row, 0, len(captures))
		for _, c := range captures {
			if len(rows) >= *limit {
				break
			}
			rows = append(rows, row{captureID(c.File), c.Name, c.CreatedAt, c.Spans, c.Models, c.File, c.InProgress})
		}
		json.NewEncoder(os.Stdout).Encode(rows)
		return
	}
	if len(captures) == 0 {
		fmt.Fprintln(os.Stderr, emptyState)
		return
	}
	for i, c := range captures {
		if i >= *limit {
			// stderr: piped stdout must stay one row per session
			fmt.Fprintf(os.Stderr, "      … and %d more — slink list --limit %d\n", len(captures)-*limit, len(captures))
			break
		}
		printSessionRow(os.Stdout, c, "")
	}
	fmt.Fprintln(os.Stderr, "\n  review:  slink view <id>\n  publish: slink share <id>")
}

func runPush(args []string) {
	fs := flag.NewFlagSet("push", flag.ExitOnError)
	yes := fs.Bool("yes", false, "publish without confirmation")
	pick := fs.Bool("pick", false, "choose from recent sessions")
	server := fs.String("server", "", "override the publish target")
	key := fs.String("key", "", "override the API key")
	setUsage(fs, "slink push [flags] [session-id | session.json]",
		"Publish a session: validate, secret-scan locally, confirm, get a link.\n  With no argument, pushes the newest local session. Scriptable; the\n  human-friendly flow is `slink share`.",
		"slink push --pick")
	parseReordered(fs, args)

	file := ""
	if ref := fs.Arg(0); ref != "" {
		var err error
		if file, err = resolveCapture(ref); err != nil {
			die(err.Error())
		}
	} else {
		captures := cli.ListCaptures(cli.CaptureDir())
		if len(captures) == 0 {
			die(emptyState + "\n  (or pass a session/v0 .json path)")
		}
		if *pick {
			file = pickSession(captures, "publish which?")
		} else {
			file = captures[0].File
			fmt.Fprintln(os.Stderr, "publishing your newest session:")
			printSessionRow(os.Stderr, captures[0], "")
		}
	}

	target, apiKey := cli.ResolveTarget(*server, *key)
	// Preflight before any bytes move: the default server always requires
	// an account, so fail here instead of after a full upload. Any custom
	// server — flag, env, or config — may allow anonymous ingest; let it
	// decide for itself.
	if apiKey == "" && target == "https://session.link" {
		die("not signed in — run: slink login  (publishing needs an account; capture doesn't)")
	}
	publishFile(file, target, apiKey, *yes)
}

// pickSession is the shared interactive picker — same renderer, same ids
// as `slink list`.
func pickSession(captures []cli.Capture, prompt string) string {
	if !term.IsTerminal(int(os.Stdin.Fd())) {
		die("--pick needs a TTY")
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
	fmt.Fprintf(os.Stderr, "%s [%s] ", prompt, captureID(captures[0].File))
	line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
	ref := strings.TrimSpace(line)
	if ref == "" {
		return captures[0].File
	}
	file, err := resolveCapture(ref)
	if err != nil {
		die(err.Error())
	}
	return file
}

// publishFile is the publish core shared by push and share: inspect, gate,
// upload, recovery footer. It exits the process on failure.
func publishFile(file, target, apiKey string, yes bool) {
	ins, err := cli.InspectRunFile(file)
	if err != nil {
		die(err.Error())
	}
	if len(ins.Errors) > 0 {
		fmt.Fprintln(os.Stderr, "✗ not a valid session/v0 document")
		fmt.Fprintf(os.Stderr, "    file:  %s\n", displayPath(file))
		for _, e := range ins.Errors {
			fmt.Fprintln(os.Stderr, "    "+e)
		}
		os.Exit(1)
	}
	name := ins.Name
	if name == "" {
		name = file
	}
	fmt.Fprintf(os.Stderr, "%q\n", clip(name, 76))
	fmt.Fprintln(os.Stderr, "  "+gateSummary(ins))
	if len(ins.Secrets) > 0 {
		fmt.Fprintln(os.Stderr, "✗ publish blocked — the session appears to contain credentials:")
		for _, h := range ins.Secrets {
			fmt.Fprintf(os.Stderr, "    %s  %s\n", h.Pattern, h.Preview)
		}
		abs, err := filepath.Abs(file)
		if err != nil {
			abs = file
		}
		fmt.Fprintf(os.Stderr, "  redact the local file and push again:\n    %s\n", abs)
		os.Exit(1)
	}
	fmt.Fprintln(os.Stderr, "✓ no credentials detected")
	if ins.InProgress {
		fmt.Fprintln(os.Stderr, "  still recording — publishing a snapshot")
	}
	if ins.Bytes > 25*1024*1024 {
		fmt.Fprintln(os.Stderr, "  exceeds the server's 25MB ingest cap — this will likely be rejected")
	}

	if !yes {
		if !term.IsTerminal(int(os.Stdin.Fd())) {
			die("not a TTY — pass --yes to publish from scripts or CI")
		}
		as := ""
		if login := cli.ReadConfig().Login; login != "" {
			as = " as @" + login
		}
		fmt.Fprintf(os.Stderr, "\nready to publish to %s%s\n", target, as)
		fmt.Fprintln(os.Stderr, "  unlisted — anyone with the link can view it")
		fmt.Fprint(os.Stderr, "\nPublish? [y/N] ")
		line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
		if l := strings.ToLower(strings.TrimSpace(line)); l != "y" && l != "yes" {
			fmt.Fprintln(os.Stderr, "aborted — nothing was uploaded")
			os.Exit(1) // scripts must be able to tell a decline from a publish
		}
	}

	res := cli.UploadRun(ins.Text, target, apiKey)
	if !res.OK {
		msg := "upload failed"
		if e, ok := res.Body["error"].(map[string]any); ok {
			if m, ok := e["message"].(string); ok {
				msg = m
			}
		}
		switch {
		case res.Status == 0: // transport failure — no HTTP happened at all
			die(fmt.Sprintf("✗ can't reach %s (%s)\n    nothing was uploaded", target, transportCause(msg)))
		case res.Status == 401:
			die(fmt.Sprintf("✗ the server refused the upload — not signed in (HTTP 401)\n\n  sign in:  slink login\n  or pass:  --key rk_…"))
		default:
			die(fmt.Sprintf("✗ %s (HTTP %d)", msg, res.Status))
		}
	}
	url, _ := res.Body["url"].(string)
	verb := "published"
	if res.Status == 200 {
		verb = "already published" // dedupe: same bytes, same URL
	}
	copied := ""
	if clipboard(url) {
		copied = "  (URL copied)"
	}
	fmt.Fprintf(os.Stderr, "✓ %s%s\n", verb, copied)
	fmt.Println(url)
	fmt.Fprintln(os.Stderr, "  anyone with this link can view it")
	if id := publishedID(url); id != "" {
		fmt.Fprintf(os.Stderr, "\n  take offline:  slink delete %s\n", id)
	}
}

// transportCause trims a wrapped Go transport error to its terminal cause
// ("connection refused", "no such host") — the URL is already on the line.
func transportCause(msg string) string {
	if i := strings.LastIndex(msg, ": "); i >= 0 {
		return msg[i+2:]
	}
	return msg
}

// gateSummary is the what-am-I-publishing facts line: spans, models, size,
// fidelity, capture age — shown under the title at the point of no return.
func gateSummary(ins *cli.Inspection) string {
	parts := []string{plural(ins.Spans, "span")}
	if len(ins.Models) > 0 {
		parts = append(parts, strings.Join(ins.Models, ", "))
	}
	parts = append(parts, fmtBytes(ins.Bytes))
	if ins.Fidelity != "" {
		parts = append(parts, ins.Fidelity)
	}
	if ins.CreatedAt != "" {
		parts = append(parts, "captured "+ago(ins.CreatedAt))
	}
	return strings.Join(parts, " · ")
}

// publishedID extracts the run id from a published URL (…/r/<id>).
func publishedID(url string) string {
	if i := strings.LastIndex(url, "/r/"); i >= 0 {
		return url[i+3:]
	}
	return ""
}

func runPrune(args []string) {
	fs := flag.NewFlagSet("prune", flag.ExitOnError)
	olderThan := fs.Int("older-than", -1, "days; sessions older are pruned")
	keep := fs.Int("keep", -1, "keep only the newest N")
	empty := fs.Bool("empty", false, "prune sessions with no recorded calls")
	yes := fs.Bool("yes", false, "prune without confirmation")
	dryRun := fs.Bool("dry-run", false, "show what would be pruned")
	setUsage(fs, "slink prune [flags]",
		"Delete old local sessions. Bare `slink prune` uses a 30-day window.",
		"slink prune --keep 50 --dry-run")
	parseReordered(fs, args)

	// Bare `slink prune` defaults to a 30-day window; --keep/--empty alone
	// don't impose an age cut (parity with the JS command).
	window := time.Duration(-1)
	switch {
	case *olderThan >= 0:
		window = time.Duration(*olderThan) * 24 * time.Hour
	case *keep < 0 && !*empty:
		window = 30 * 24 * time.Hour
	}
	captures := cli.ListCaptures(cli.CaptureDir())
	remove, _ := cli.PlanPrune(captures, time.Now(), window, *keep, *empty)
	if len(remove) == 0 {
		fmt.Fprintln(os.Stderr, "nothing to prune")
		return
	}
	var bytes int64
	for i, c := range remove {
		if st, err := os.Stat(c.File); err == nil {
			bytes += st.Size()
		}
		if i < 8 { // cap the listing like the JS command
			printSessionRow(os.Stderr, c, "")
		} else if i == 8 {
			fmt.Fprintf(os.Stderr, "  … and %d more\n", len(remove)-8)
		}
	}
	fmt.Fprintf(os.Stderr, "\n  reclaims:  %s\n", fmtBytes(int(bytes)))
	if *dryRun {
		fmt.Fprintf(os.Stderr, "dry run — %s would be pruned\n", plural(len(remove), "session"))
		return
	}
	if !*yes {
		if !term.IsTerminal(int(os.Stdin.Fd())) {
			die("not a TTY — pass --yes to prune from scripts")
		}
		fmt.Fprintf(os.Stderr, "\nPrune %s? [y/N] ", plural(len(remove), "session"))
		line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
		if l := strings.ToLower(strings.TrimSpace(line)); l != "y" && l != "yes" {
			fmt.Fprintln(os.Stderr, "aborted")
			return
		}
	}
	for _, c := range remove {
		cli.RemoveCapture(c.File)
	}
	fmt.Fprintf(os.Stderr, "pruned %s\n", plural(len(remove), "session"))
}

/* -------------------------------------------------------------- helpers */

func die(msg string) {
	fmt.Fprintln(os.Stderr, msg)
	os.Exit(1)
}

func clip(s string, n int) string {
	r := []rune(s)
	if len(r) <= n {
		return s
	}
	return string(r[:n-1]) + "…"
}

func ago(iso string) string {
	t, err := time.Parse(time.RFC3339, iso)
	if err != nil {
		return "?"
	}
	d := time.Since(t)
	if d < 0 {
		return "just now" // clock skew / future stamps — never "-204s ago"
	}
	switch {
	case d < time.Minute:
		return fmt.Sprintf("%ds ago", int(d.Seconds()))
	case d < time.Hour:
		return fmt.Sprintf("%dm ago", int(d.Minutes()))
	case d < 24*time.Hour:
		return fmt.Sprintf("%dh ago", int(d.Hours()))
	default:
		return fmt.Sprintf("%dd ago", int(d.Hours()/24))
	}
}

func fmtBytes(n int) string {
	if n >= 1e6 {
		return fmt.Sprintf("%.1f MB", float64(n)/1e6)
	}
	kb := n / 1024
	if kb < 1 {
		kb = 1
	}
	return fmt.Sprintf("%d KB", kb)
}

func clipboard(text string) bool {
	if text == "" {
		return false
	}
	var candidates [][]string
	switch runtime.GOOS {
	case "darwin":
		candidates = [][]string{{"pbcopy"}}
	case "windows":
		candidates = [][]string{{"clip"}}
	default:
		candidates = [][]string{{"wl-copy"}, {"xclip", "-selection", "clipboard"}}
	}
	for _, c := range candidates {
		cmd := exec.Command(c[0], c[1:]...)
		cmd.Stdin = strings.NewReader(text)
		if cmd.Run() == nil {
			return true
		}
	}
	return false
}
