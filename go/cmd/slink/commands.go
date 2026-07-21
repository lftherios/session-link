package main

import (
	"bufio"
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

// emptyState is the shared "no sessions yet" message — every surface that
// can hit an empty store points at the same three ways in.
const emptyState = "no sessions yet — record one (slink dev -- <cmd>), import one (slink import), or go always-on (slink tap)"

func runList(args []string) {
	fs := flag.NewFlagSet("list", flag.ExitOnError)
	limit := fs.Int("limit", 10, "rows to show")
	setUsage(fs, "slink list [flags]",
		"List local sessions, newest first. Nothing here has been published.",
		"slink list --limit 25")
	parseReordered(fs, args)
	if *limit < 1 {
		*limit = 1
	}
	captures := cli.ListCaptures(cli.CaptureDir())
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
		marker := ""
		if c.InProgress {
			marker = "  (recording)"
		}
		fmt.Printf("%4d  %-9s %-38s %d spans  %s%s\n",
			i+1, ago(c.CreatedAt), clip(c.Name, 38), c.Spans, strings.Join(c.Models, ", "), marker)
	}
	fmt.Fprintln(os.Stderr, "\n  review: slink open · publish: slink push")
}

func runPush(args []string) {
	fs := flag.NewFlagSet("push", flag.ExitOnError)
	yes := fs.Bool("yes", false, "publish without confirmation")
	pick := fs.Bool("pick", false, "choose from recent sessions")
	server := fs.String("server", "", "override the publish target")
	key := fs.String("key", "", "override the API key")
	setUsage(fs, "slink push [flags] [session.json]",
		"Publish a session: validate, secret-scan locally, confirm, get a link.\n  With no path, pushes the newest local session.",
		"slink push --pick")
	parseReordered(fs, args)

	file := fs.Arg(0)
	if file == "" {
		captures := cli.ListCaptures(cli.CaptureDir())
		if len(captures) == 0 {
			die(emptyState + "\n  (or pass a session/v0 .json path)")
		}
		if *pick {
			if !term.IsTerminal(int(os.Stdin.Fd())) {
				die("--pick needs a TTY")
			}
			for i, cc := range captures {
				if i >= 10 {
					break
				}
				marker := ""
				if cc.InProgress {
					marker = "  (recording)"
				}
				fmt.Fprintf(os.Stderr, "%4d  %-9s %-38s %d spans%s\n", i+1, ago(cc.CreatedAt), clip(cc.Name, 38), cc.Spans, marker)
			}
			fmt.Fprint(os.Stderr, "publish which? [1] ")
			line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
			n := 1
			if t := strings.TrimSpace(line); t != "" {
				fmt.Sscanf(t, "%d", &n)
			}
			if n < 1 || n > len(captures) {
				die(fmt.Sprintf("no capture #%d", n))
			}
			file = captures[n-1].File
		} else {
			file = captures[0].File
		}
	}

	ins, err := cli.InspectRunFile(file)
	if err != nil {
		die(err.Error())
	}
	if len(ins.Errors) > 0 {
		fmt.Fprintln(os.Stderr, "✗ not a valid session/v0 document:")
		for _, e := range ins.Errors {
			fmt.Fprintln(os.Stderr, "    "+e)
		}
		os.Exit(1)
	}
	name := ins.Name
	if name == "" {
		name = file
	}
	fmt.Fprintf(os.Stderr, "%s · %d spans · %s · %s\n", name, ins.Spans, strings.Join(ins.Models, ", "), fmtBytes(ins.Bytes))
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

	target, apiKey := cli.ResolveTarget(*server, *key)
	if !*yes {
		if !term.IsTerminal(int(os.Stdin.Fd())) {
			die("not a TTY — pass --yes to publish from scripts or CI")
		}
		fmt.Fprintf(os.Stderr, "Publish unlisted to %s? [y/N] ", target)
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
		if res.Status == 401 {
			msg += " — run `slink login` (publishing needs an account; capture doesn't), or pass --key"
		}
		die(fmt.Sprintf("✗ %s (HTTP %d)", msg, res.Status))
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
			fmt.Fprintf(os.Stderr, "  %-9s %s (%d spans)\n", ago(c.CreatedAt), clip(c.Name, 48), c.Spans)
		} else if i == 8 {
			fmt.Fprintf(os.Stderr, "  … and %d more\n", len(remove)-8)
		}
	}
	fmt.Fprintf(os.Stderr, "  reclaims %s\n", fmtBytes(int(bytes)))
	if *dryRun {
		fmt.Fprintf(os.Stderr, "dry run — %d capture(s) would be pruned\n", len(remove))
		return
	}
	if !*yes {
		if !term.IsTerminal(int(os.Stdin.Fd())) {
			die("not a TTY — pass --yes to prune from scripts")
		}
		fmt.Fprintf(os.Stderr, "Prune %d capture(s)? [y/N] ", len(remove))
		line, _ := bufio.NewReader(os.Stdin).ReadString('\n')
		if l := strings.ToLower(strings.TrimSpace(line)); l != "y" && l != "yes" {
			fmt.Fprintln(os.Stderr, "aborted")
			return
		}
	}
	for _, c := range remove {
		cli.RemoveCapture(c.File)
	}
	fmt.Fprintf(os.Stderr, "pruned %d capture(s)\n", len(remove))
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
