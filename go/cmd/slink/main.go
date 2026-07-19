// The Go slink CLI — being ported per docs/go-migration.md. Until parity
// holds across the CLI surface, the JS CLI remains the shipped default;
// the tap daemon (P1) lands here first.
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

var version = "0.0.0-dev" // stamped by goreleaser at P4

func main() {
	if len(os.Args) < 2 {
		usage()
	}
	switch os.Args[1] {
	case "version":
		fmt.Printf("slink %s (go)\n", version)
	case "tap":
		runTap(os.Args[2:])
	case "list", "ls":
		runList(os.Args[2:])
	case "push":
		runPush(os.Args[2:])
	case "prune":
		runPrune(os.Args[2:])
	case "login":
		runLogin(os.Args[2:])
	case "on":
		runOn(os.Args[2:])
	case "off":
		runOff()
	default:
		usage()
	}
}

func usage() {
	fmt.Fprintln(os.Stderr, "slink(go): port in progress — tap [--install], list, push, prune, login, on/off, version; use the JS CLI for the rest")
	os.Exit(1)
}

func runTap(args []string) {
	fs := flag.NewFlagSet("tap", flag.ExitOnError)
	port := fs.Int("port", 4141, "listen port")
	idleMin := fs.Int("idle", 15, "ambient session idle gap, minutes")
	install := fs.Bool("install", false, "install as a login service (launchd/systemd)")
	uninstall := fs.Bool("uninstall", false, "remove the login service")
	fs.Parse(args)

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
