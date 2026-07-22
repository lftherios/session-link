package main

import (
	"flag"
	"fmt"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"golang.org/x/term"
)

// tapReachable is true only for a real slink tap — any other HTTP server
// squatting the port must not be handed LLM traffic.
func tapReachable(port int) bool {
	_, ok := tapInfo(port)
	return ok
}

// startTapDaemon starts the tap detached so it outlives this invocation
// (and the shell that ran it), logging to ~/.slink/tap.log. extraEnv lets
// the caller hand the daemon upstream overrides (SLINK_UPSTREAM_*).
func startTapDaemon(port int, extraEnv []string) error {
	exe, err := os.Executable()
	if err != nil {
		return err
	}
	logPath := cli.TapLogPath()
	os.MkdirAll(filepath.Dir(logPath), 0o755)
	out, err := os.OpenFile(logPath, os.O_APPEND|os.O_CREATE|os.O_WRONLY, 0o644)
	if err != nil {
		return err
	}
	defer out.Close()
	cmd := exec.Command(exe, "tap", "--port", fmt.Sprintf("%d", port))
	cmd.Env = append(os.Environ(), extraEnv...)
	cmd.Stdout = out
	cmd.Stderr = out
	detach(cmd)
	if err := cmd.Start(); err != nil {
		return err
	}
	return cmd.Process.Release()
}

// customUpstream returns a BASE_URL the user had already pointed somewhere
// of their own — e.g. Ollama — and "" when it's unset, already a slink
// tap/proxy address, or just the vendor default spelled out. Routing a
// shell with a custom endpoint through the tap without carrying the
// upstream along would silently send that traffic to the default vendor
// instead.
func customUpstream(env string) string {
	v := os.Getenv(env)
	if v == "" {
		return ""
	}
	u, err := url.Parse(v)
	if err != nil {
		return v
	}
	host := u.Hostname()
	if (host == "127.0.0.1" || host == "localhost") &&
		(strings.HasPrefix(u.Path, "/anthropic") || strings.HasPrefix(u.Path, "/openai")) {
		return "" // already routed through a slink tap — safe to re-point
	}
	if host == "api.anthropic.com" || host == "api.openai.com" {
		return "" // the vendor default, just written out — not a custom route
	}
	return v
}

// tapShaped reports whether an env var currently points at a slink tap —
// the only kind of value on/off ever wrote, and the only kind off may
// remove.
func tapShaped(env string) bool {
	return os.Getenv(env) != "" && customUpstream(env) == ""
}

// runOn: `eval "$(slink on)"` — only export lines go to stdout so eval
// stays clean; hints go to stderr. Honesty rules: never export when the
// tap isn't actually up, and never claim capture is on unless it can be.
func runOn(args []string) {
	fs := flag.NewFlagSet("on", flag.ExitOnError)
	port := fs.Int("port", 0, "tap port (default: the running tap's, else 4141)")
	noStart := fs.Bool("no-start", false, "don't auto-start the tap")
	setUsage(fs, `eval "$(slink on)"  [flags]`,
		"Route this shell's Anthropic/OpenAI traffic through the always-on tap.\n  Prints export lines — they only take effect inside eval \"$(...)\".",
		`eval "$(slink on)"`)
	parseReordered(fs, args)
	if *port == 0 {
		if *port = persistedTapPort(); *port == 0 {
			*port = 4141
		}
	}

	prevAnthropic := customUpstream("ANTHROPIC_BASE_URL")
	prevOpenAI := customUpstream("OPENAI_BASE_URL")

	up := tapReachable(*port)
	startedHere := false
	if !up {
		if *noStart {
			fmt.Fprintf(os.Stderr, "# tap not running on :%d — nothing exported\n", *port)
			fmt.Fprintf(os.Stderr, "# start it (slink tap), then rerun: eval \"$(slink on)\"\n")
			os.Exit(1)
		}
		// Carry the shell's own endpoints into the daemon so routing through
		// the tap never silently redirects traffic to the default vendors.
		// An explicit SLINK_UPSTREAM_* wins; note the real source either way.
		var extra []string
		srcAnthropic, srcOpenAI := "", ""
		if v := os.Getenv("SLINK_UPSTREAM_ANTHROPIC"); v != "" {
			srcAnthropic = v + " (from SLINK_UPSTREAM_ANTHROPIC)"
		} else if prevAnthropic != "" {
			extra = append(extra, "SLINK_UPSTREAM_ANTHROPIC="+prevAnthropic)
			srcAnthropic = prevAnthropic + " (kept from ANTHROPIC_BASE_URL)"
		}
		if v := os.Getenv("SLINK_UPSTREAM_OPENAI"); v != "" {
			srcOpenAI = v + " (from SLINK_UPSTREAM_OPENAI)"
		} else if prevOpenAI != "" {
			extra = append(extra, "SLINK_UPSTREAM_OPENAI="+prevOpenAI)
			srcOpenAI = prevOpenAI + " (kept from OPENAI_BASE_URL)"
		}
		if err := startTapDaemon(*port, extra); err != nil {
			fmt.Fprintf(os.Stderr, "# could not start the tap: %v — nothing exported\n", err)
			os.Exit(1)
		}
		for deadline := time.Now().Add(2500 * time.Millisecond); time.Now().Before(deadline); {
			if tapReachable(*port) {
				up = true
				break
			}
			time.Sleep(100 * time.Millisecond)
		}
		if !up {
			fmt.Fprintf(os.Stderr, "# tap didn't come up on :%d — nothing exported\n", *port)
			fmt.Fprintf(os.Stderr, "#   check the log:  %s\n", displayPath(cli.TapLogPath()))
			os.Exit(1)
		}
		startedHere = true
		fmt.Fprintf(os.Stderr, "# started a background tap on :%d (log: %s)\n", *port, cli.TapLogPath())
		if srcAnthropic != "" {
			fmt.Fprintf(os.Stderr, "# it forwards Anthropic-style calls to %s\n", srcAnthropic)
		}
		if srcOpenAI != "" {
			fmt.Fprintf(os.Stderr, "# it forwards OpenAI-style calls to %s\n", srcOpenAI)
		}
	}

	// An already-running tap forwards to whatever upstream it was started
	// with — we can't know if that matches this shell's custom endpoint, so
	// refuse to reroute that provider rather than risk sending its traffic
	// (prompts included) to the default vendor.
	var routed, skipped []string
	skip := func(env, prev, slinkVar string) bool {
		if startedHere || prev == "" {
			return false
		}
		fmt.Fprintf(os.Stderr, "# %s points at %s — leaving it alone so that traffic keeps going there\n", env, prev)
		fmt.Fprintf(os.Stderr, "# to record it too: stop the running tap, then  %s=%s slink tap\n", slinkVar, prev)
		return true
	}
	if skip("ANTHROPIC_BASE_URL", prevAnthropic, "SLINK_UPSTREAM_ANTHROPIC") {
		skipped = append(skipped, "Anthropic")
	} else {
		fmt.Printf("export ANTHROPIC_BASE_URL=http://127.0.0.1:%d/anthropic\n", *port)
		routed = append(routed, "Anthropic")
	}
	if skip("OPENAI_BASE_URL", prevOpenAI, "SLINK_UPSTREAM_OPENAI") {
		skipped = append(skipped, "OpenAI")
	} else {
		fmt.Printf("export OPENAI_BASE_URL=http://127.0.0.1:%d/openai/v1\n", *port)
		routed = append(routed, "OpenAI")
	}

	switch {
	case len(routed) == 0:
		fmt.Fprintln(os.Stderr, "# nothing exported — this shell keeps its own endpoints")
		os.Exit(1)
	case term.IsTerminal(int(os.Stdout.Fd())):
		// Printed to a terminal, not eval'd — the exports above changed nothing.
		fmt.Fprintln(os.Stderr, `# nothing changed yet — run: eval "$(slink on)"`)
	case len(skipped) > 0:
		fmt.Fprintf(os.Stderr, "# capture is on for %s calls in this shell (%s left untouched)\n",
			strings.Join(routed, "+"), strings.Join(skipped, "+"))
		fmt.Fprintln(os.Stderr, "#   review:   slink view")
		fmt.Fprintln(os.Stderr, "#   publish:  slink share")
	default:
		fmt.Fprintln(os.Stderr, "# capture is on for this shell")
		fmt.Fprintln(os.Stderr, "#   review:   slink view")
		fmt.Fprintln(os.Stderr, "#   publish:  slink share")
	}
}

func runOff(args []string) {
	fs := flag.NewFlagSet("off", flag.ExitOnError)
	setUsage(fs, `eval "$(slink off)"`,
		"Stop routing this shell through the tap. Prints unset lines — they\n  only take effect inside eval \"$(...)\".",
		`eval "$(slink off)"`)
	parseReordered(fs, args)

	// Only remove what on(-style routing) wrote: a custom endpoint the user
	// set themselves (Ollama, a gateway) is not ours to delete.
	unset := 0
	for _, env := range []string{"ANTHROPIC_BASE_URL", "OPENAI_BASE_URL"} {
		switch {
		case tapShaped(env):
			fmt.Printf("unset %s\n", env)
			unset++
		case os.Getenv(env) != "":
			fmt.Fprintf(os.Stderr, "# %s points at %s — not slink's, leaving it alone\n", env, os.Getenv(env))
		default:
			fmt.Printf("unset %s\n", env) // unset twice is harmless; keeps eval output stable
		}
	}
	switch {
	case term.IsTerminal(int(os.Stdout.Fd())):
		fmt.Fprintln(os.Stderr, `# nothing changed yet — run: eval "$(slink off)"`)
	case unset == 0:
		fmt.Fprintln(os.Stderr, "# capture was already off for this shell")
	default:
		fmt.Fprintln(os.Stderr, "# capture off for this shell")
	}
}

func runLogin(args []string) {
	fs := flag.NewFlagSet("login", flag.ExitOnError)
	key := fs.String("key", "", "paste an rk_… API key instead of the browser flow")
	server := fs.String("server", "", "login against a specific server")
	setUsage(fs, "slink login [flags]",
		"One-time sign-in (GitHub) so published sessions are attributed to you\n  and deletable by you. Capturing never needs an account.",
		"slink login")
	parseReordered(fs, args)

	if *key != "" {
		r, err := cli.LoginWithKey(*key, *server)
		if err != nil {
			die(err.Error())
		}
		fmt.Fprintf(os.Stderr, "✓ saved to %s\n", r.ConfigPath)
		return
	}
	target, _ := cli.ResolveTarget(*server, "")
	r, err := cli.BrowserLogin(target, func(line string) { fmt.Fprintln(os.Stderr, line) })
	if err != nil {
		die(err.Error())
	}
	fmt.Fprintf(os.Stderr, "✓ logged in as @%s to %s — saved to %s\n", r.Login, target, r.ConfigPath)
	if r.PrevServer != "" && r.PrevServer != target {
		fmt.Fprintf(os.Stderr, "  note: default push target changed from %s to %s\n", r.PrevServer, target)
	}
}
