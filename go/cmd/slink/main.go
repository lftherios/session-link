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
func dispatch(cmd string, args []string) bool {
	switch cmd {
	case "tap":
		runTap(args)
	case "list", "ls":
		runList(args)
	case "push":
		runPush(args)
	case "prune":
		runPrune(args)
	case "login":
		runLogin(args)
	case "on":
		runOn(args)
	case "off":
		runOff(args)
	case "open":
		runOpen(args)
	case "dev":
		runDev(args)
	case "import":
		runImport(args)
	case "share":
		runShare(args)
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
	setUsage(fs, "slink tap [flags]",
		"Run the always-on recorder in the foreground. Shells route through it\n  via `eval \"$(slink on)\"`; each burst of traffic becomes its own session.",
		"slink tap --port 4141")
	parseReordered(fs, args)

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

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()

	srv := tap.NewServer(dir, time.Duration(*idleMin)*time.Minute)
	if err := srv.Serve(ctx, fmt.Sprintf("127.0.0.1:%d", *port)); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
