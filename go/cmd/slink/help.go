package main

import (
	"flag"
	"fmt"
	"io"
	"os"
	"strings"
)

// commands is the dispatch list usage and did-you-mean draw from; keep it
// in sync with the switch in main().
var commands = []string{
	"share", "record", "dev", "view", "open", "list", "ls",
	"import", "tap", "setup", "on", "off", "status",
	"login", "logout", "push", "delete",
	"prune", "doctor", "completion", "version", "help",
}

func printUsage(w io.Writer) {
	fmt.Fprint(w, `slink — record LLM sessions locally, share the ones worth sharing.

Usage:
  slink <command> [flags]

Share
  share              publish the session you mean — newest local or your
                     coding agent's newest — review, confirm, get a link
  delete             take a published session offline

Record (local — nothing leaves your machine)
  record -- <cmd>    run a command with its LLM calls recorded  (alias: dev)
  import             convert your coding agent's newest session — no re-run
                     (claude-code, codex, opencode, pi, hermes)
  setup              guided always-on capture (tap as a login service)
  on / off           route this shell through the tap:  eval "$(slink on)"

Review
  list               your local sessions, newest first (ids in column one)
  view               browse them in the local viewer  (alias: open)
  status             recorder, routing, upstreams, account, sessions

Publish plumbing (share does all of this for you)
  login / logout     account for publishing; capture never needs one
  push               validate, secret-scan, confirm → link (scriptable)

Maintain
  tap                the always-on recorder (foreground; --stop, --install)
  prune              delete old local sessions
  doctor             check the recording setup end to end
  completion         shell tab-completion (bash, zsh, fish)
  version            print the slink version

Quickstart:
  slink record -- python agent.py   # 1. record a run   (or: slink import)
  slink view                        # 2. review it locally
  slink share                       # 3. publish → https://session.link/r/…

Flags for a command: slink <command> -h
Sessions stay local in ~/.slink until you publish them (the always-on tap
prunes its old ones after 30 days; one-off recordings are kept).
`)
}

// setUsage gives a subcommand's -h a synopsis with positionals, a blurb,
// and an example — flag.PrintDefaults alone hides the argument grammar.
func setUsage(fs *flag.FlagSet, synopsis, blurb, example string) {
	fs.Usage = func() {
		w := fs.Output()
		fmt.Fprintf(w, "Usage:\n  %s\n\n  %s\n", synopsis, blurb)
		fmt.Fprintln(w, "\nFlags:")
		fs.PrintDefaults()
		if example != "" {
			fmt.Fprintf(w, "\nExample:\n  %s\n", example)
		}
	}
}

func unknownCommand(cmd string) {
	fmt.Fprintf(os.Stderr, "slink: unknown command %q\n", cmd)
	if s := suggest(cmd); s != "" {
		fmt.Fprintf(os.Stderr, "  did you mean: slink %s?\n", s)
	}
	fmt.Fprintln(os.Stderr, "  run `slink --help` for the command list")
	os.Exit(1)
}

// suggest returns the closest command within edit distance 2, or "".
func suggest(cmd string) string {
	best, bestDist := "", 3
	for _, c := range commands {
		if d := editDistance(cmd, c); d < bestDist {
			best, bestDist = c, d
		}
	}
	return best
}

func editDistance(a, b string) int {
	ra, rb := []rune(a), []rune(b)
	prev := make([]int, len(rb)+1)
	cur := make([]int, len(rb)+1)
	for j := range prev {
		prev[j] = j
	}
	for i := 1; i <= len(ra); i++ {
		cur[0] = i
		for j := 1; j <= len(rb); j++ {
			cost := 1
			if ra[i-1] == rb[j-1] {
				cost = 0
			}
			cur[j] = min3(prev[j]+1, cur[j-1]+1, prev[j-1]+cost)
		}
		prev, cur = cur, prev
	}
	return prev[len(rb)]
}

func min3(a, b, c int) int {
	if b < a {
		a = b
	}
	if c < a {
		a = c
	}
	return a
}

// parseReordered parses args with positionals allowed before flags — Go's
// flag package stops at the first non-flag token, which silently dropped
// e.g. `slink push file.json --yes`. Flags are recognized from the FlagSet
// itself (bool flags don't consume a value); everything after a literal
// "--" stays positional, verbatim. An explicit help flag prints usage to
// stdout and exits 0, so `slink push -h` matches `slink --help`.
func parseReordered(fs *flag.FlagSet, args []string) {
	known := map[string]bool{}
	isBool := map[string]bool{}
	fs.VisitAll(func(f *flag.Flag) {
		known["-"+f.Name] = true
		known["--"+f.Name] = true
		if bv, ok := f.Value.(interface{ IsBoolFlag() bool }); ok && bv.IsBoolFlag() {
			isBool["-"+f.Name] = true
			isBool["--"+f.Name] = true
		}
	})
	var flags, pos []string
	for i := 0; i < len(args); i++ {
		a := args[i]
		if a == "--" {
			pos = append(pos, args[i+1:]...)
			break
		}
		switch a {
		case "-h", "--help", "-help":
			fs.SetOutput(os.Stdout)
			fs.Usage()
			os.Exit(0)
		}
		if strings.HasPrefix(a, "-") && a != "-" {
			flags = append(flags, a)
			name := strings.SplitN(a, "=", 2)[0]
			if known[name] && !isBool[name] && !strings.Contains(a, "=") {
				// A value-flag must actually have a value — otherwise the
				// "--" sentinel below (or nothing) would silently become it.
				if i+1 >= len(args) || args[i+1] == "--" {
					fmt.Fprintf(os.Stderr, "flag needs an argument: %s\n", a)
					fs.Usage()
					os.Exit(2)
				}
				flags = append(flags, args[i+1])
				i++
			}
		} else {
			pos = append(pos, a)
		}
	}
	fs.Parse(append(append(flags, "--"), pos...))
}

func plural(n int, word string) string {
	if n == 1 {
		return fmt.Sprintf("%d %s", n, word)
	}
	return fmt.Sprintf("%d %ss", n, word)
}
