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
	default:
		usage()
	}
}

func usage() {
	fmt.Fprintln(os.Stderr, "slink(go): port in progress — tap, list, push, prune, version; use the JS CLI for the rest")
	os.Exit(1)
}

func runTap(args []string) {
	fs := flag.NewFlagSet("tap", flag.ExitOnError)
	port := fs.Int("port", 4141, "listen port")
	idleMin := fs.Int("idle", 15, "ambient session idle gap, minutes")
	fs.Parse(args)

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
