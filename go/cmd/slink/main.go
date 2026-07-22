// The slink CLI — record LLM sessions locally (dev/tap), review them
// (list/open), and publish the ones worth sharing (login/push/share).
package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/tap"
)

var version = "0.0.0-dev" // stamped by goreleaser

func main() {
	if len(os.Args) < 2 {
		printUsage(os.Stderr)
		os.Exit(1)
	}
	cmd, rest := os.Args[1], os.Args[2:]
	switch cmd {
	case "help", "--help", "-h", "-help":
		if len(rest) > 0 && dispatch(rest[0], []string{"-h"}) {
			return
		}
		printUsage(os.Stdout)
	case "version", "--version", "-v", "-version":
		fmt.Printf("slink %s\n", version)
	default:
		if !dispatch(cmd, rest) {
			unknownCommand(cmd)
		}
	}
}

// dispatch runs a named subcommand; false means the name is unknown.
// record/view are the canonical names; dev/open remain as aliases.
func dispatch(cmd string, args []string) bool {
	switch cmd {
	case "tap":
		runTap(args)
	case "setup":
		runSetup(args)
	case "status":
		runStatus(args)
	case "list", "ls":
		runList(args)
	case "push":
		runPush(args)
	case "prune":
		runPrune(args)
	case "login":
		runLogin(args)
	case "logout":
		runLogout(args)
	case "on":
		runOn(args)
	case "off":
		runOff(args)
	case "view", "open":
		runOpen(args)
	case "record", "dev":
		runDev(args)
	case "import":
		runImport(args)
	case "share":
		runShare(args)
	case "delete":
		runDelete(args)
	case "doctor":
		runDoctor(args)
	case "completion":
		runCompletion(args)
	default:
		return false
	}
	return true
}

func runTap(args []string) {
	fs := flag.NewFlagSet("tap", flag.ExitOnError)
	port := fs.Int("port", 4141, "listen port")
	idleMin := fs.Int("idle", 15, "minutes of quiet before any session rolls into a new activity window")
	install := fs.Bool("install", false, "install as a login service (launchd/systemd)")
	uninstall := fs.Bool("uninstall", false, "remove the login service")
	stop := fs.Bool("stop", false, "stop the running tap daemon")
	setUsage(fs, "slink tap [flags]",
		"Run the always-on recorder in the foreground. Shells route through it\n  via `eval \"$(slink on)\"`; each burst of traffic becomes its own session.",
		"slink tap --port 4141")
	parseReordered(fs, args)

	if *stop {
		stopTap()
		return
	}
	if *install || *uninstall {
		var r cli.ServiceResult
		if *uninstall {
			r = cli.UninstallService()
		} else {
			r = cli.InstallService(*port)
		}
		if !r.OK {
			fmt.Fprintln(os.Stderr, r.Error)
			os.Exit(1)
		}
		if *uninstall {
			fmt.Fprintf(os.Stderr, "✓ tap login service removed  %s\n", r.Path)
		} else {
			fmt.Fprintf(os.Stderr, "✓ tap installed as a login service — it captures on every login\n  %s\n", r.Path)
		}
		return
	}

	home := os.Getenv("SLINK_HOME")
	if home == "" {
		h, err := os.UserHomeDir()
		if err != nil {
			fmt.Fprintln(os.Stderr, "cannot resolve home dir:", err)
			os.Exit(1)
		}
		home = filepath.Join(h, ".slink")
	}
	dir := filepath.Join(home, "runs")

	// A tap already answering — on this port or another — means a second
	// daemon would fork the capture stream (or fail to bind and then
	// clobber the running tap's pid/port files). Refuse both, before any
	// file is touched.
	if _, up := tapInfo(*port); up {
		fmt.Fprintf(os.Stderr, "a tap is already running on :%d\n  stop it first: slink tap --stop\n", *port)
		os.Exit(1)
	}
	if prev := persistedTapPort(); prev != 0 && prev != *port {
		if _, up := tapInfo(prev); up {
			fmt.Fprintf(os.Stderr, "a tap is already running on :%d — this one (:%d) would split your captures\n", prev, *port)
			fmt.Fprintf(os.Stderr, "  stop it first: slink tap --stop\n")
			os.Exit(1)
		}
	}

	ctx, stopSig := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stopSig()

	writeTapFiles(*port)
	defer removeTapFiles()
	srv := tap.NewServer(dir, time.Duration(*idleMin)*time.Minute)
	if err := srv.Serve(ctx, fmt.Sprintf("127.0.0.1:%d", *port)); err != nil {
		removeTapFiles()
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

// runSetup is the guided path into always-on capture: explain, confirm,
// install the login service, and say how to verify and undo.
func runSetup(args []string) {
	fs := flag.NewFlagSet("setup", flag.ExitOnError)
	port := fs.Int("port", 4141, "tap port")
	setUsage(fs, "slink setup",
		"Guided always-on capture: installs the tap as a login service so\n  recording is a background fact — local only, prunable, undoable.",
		"slink setup")
	parseReordered(fs, args)

	fmt.Fprintln(os.Stderr, `This installs the slink tap as a login service:
  · it records LLM traffic that shells route to it — locally, to ~/.slink
  · nothing is uploaded unless you run slink share / slink push
  · sessions older than 30 days are cleaned up (each time the tap starts)
  · undo anytime: slink tap --uninstall`)
	fmt.Fprint(os.Stderr, "Install? [y/N] ")
	var line string
	fmt.Fscanln(os.Stdin, &line)
	if l := strings.ToLower(strings.TrimSpace(line)); l != "y" && l != "yes" {
		fmt.Fprintln(os.Stderr, "aborted — nothing installed")
		fmt.Fprintln(os.Stderr, "\n  one-off recording works anytime:  slink record -- <cmd>")
		os.Exit(1)
	}
	r := cli.InstallService(*port)
	if !r.OK {
		die(r.Error)
	}
	fmt.Fprintf(os.Stderr, "✓ installed  %s\n", r.Path)
	// InstallService loads the service immediately (launchctl load /
	// systemctl --now) — wait for it rather than racing it with a second,
	// unmanaged daemon the service manager doesn't know about.
	up := false
	for deadline := time.Now().Add(4 * time.Second); time.Now().Before(deadline); {
		if _, ok := tapInfo(*port); ok {
			up = true
			break
		}
		time.Sleep(150 * time.Millisecond)
	}
	if up {
		fmt.Fprintf(os.Stderr, "✓ tap running on :%d\n", *port)
	} else {
		fmt.Fprintf(os.Stderr, "the tap hasn't come up yet — check in a moment with: slink status  (log: %s)\n", cli.TapLogPath())
	}
	fmt.Fprintln(os.Stderr, `next:
  route this shell:   eval "$(slink on)"
  check anytime:      slink status
  publish something:  slink share`)
}
