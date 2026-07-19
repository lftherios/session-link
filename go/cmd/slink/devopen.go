package main

import (
	"context"
	"flag"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"strings"
	"syscall"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/open"
	"github.com/lftherios/session-link/internal/tap"
)

func runOpen(args []string) {
	fs := flag.NewFlagSet("open", flag.ExitOnError)
	port := fs.Int("port", 4400, "listen port")
	noBrowser := fs.Bool("no-browser", false, "don't open the browser")
	fs.Parse(args)
	target, apiKey := cli.ResolveTarget("", "")
	srv := &open.Server{CaptureDir: cli.CaptureDir(), Target: target, APIKey: apiKey}
	addr, _, err := srv.Serve(*port)
	if err != nil {
		die(err.Error())
	}
	fmt.Fprintf(os.Stderr, "session.link local viewer · %s · publishes to %s\n", addr, target)
	if !*noBrowser {
		cli.OpenBrowser(addr)
	}
	select {} // serve until interrupted
}

// runDev is wrapper mode: record one session around a command (or until
// Ctrl-C without one), then finalize and summarize.
func runDev(args []string) {
	fs := flag.NewFlagSet("dev", flag.ExitOnError)
	port := fs.Int("port", -1, "listen port (default: ephemeral when wrapping, 4141 otherwise)")
	name := fs.String("name", "", "session name")
	fs.Parse(args)
	rest := fs.Args()
	var cmdArgs []string
	for i, a := range rest {
		if a == "--" {
			cmdArgs = rest[i+1:]
			break
		}
	}
	if cmdArgs == nil && len(rest) > 0 {
		cmdArgs = rest // `slink dev -- cmd` puts cmd after --; tolerate without
	}

	sessName := *name
	if sessName == "" {
		if len(cmdArgs) > 0 {
			sessName = strings.Join(cmdArgs, " ")
		} else {
			sessName = "proxy session"
		}
	}
	srv, session := tap.NewDevServer(cli.CaptureDir(), sessName)

	listenPort := *port
	if listenPort < 0 {
		if len(cmdArgs) > 0 {
			listenPort = 0 // wrapper mode: ephemeral
		} else {
			listenPort = 4141
		}
	}
	ln, err := net.Listen("tcp", fmt.Sprintf("127.0.0.1:%d", listenPort))
	if err != nil {
		die(err.Error())
	}
	httpSrv := &http.Server{Handler: srv}
	go httpSrv.Serve(ln)
	p := ln.Addr().(*net.TCPAddr).Port
	env := []string{
		fmt.Sprintf("ANTHROPIC_BASE_URL=http://127.0.0.1:%d/anthropic", p),
		fmt.Sprintf("OPENAI_BASE_URL=http://127.0.0.1:%d/openai/v1", p),
	}
	fmt.Fprintf(os.Stderr, "session.link proxy · http://127.0.0.1:%d · recording %s\n", p, session.File)

	exitCode := 0
	if len(cmdArgs) > 0 {
		child := exec.Command(cmdArgs[0], cmdArgs[1:]...)
		child.Stdin, child.Stdout, child.Stderr = os.Stdin, os.Stdout, os.Stderr
		child.Env = append(os.Environ(), env...)
		// Ctrl-C goes to the child (same process group); we finalize after.
		signal.Ignore(os.Interrupt)
		if err := child.Run(); err != nil {
			if ee, ok := err.(*exec.ExitError); ok {
				exitCode = ee.ExitCode()
			} else {
				fmt.Fprintf(os.Stderr, "failed to start %s: %v\n", cmdArgs[0], err)
				exitCode = 1
			}
		}
	} else {
		for _, e := range env {
			fmt.Printf("export %s\n", e)
		}
		fmt.Fprintln(os.Stderr, "Ctrl-C to finish")
		ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
		<-ctx.Done()
		stop()
		fmt.Fprintln(os.Stderr)
	}

	shutCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	httpSrv.Shutdown(shutCtx)
	cancel()
	httpSrv.Close()
	session.Finalize()
	fmt.Fprintln(os.Stderr, "publish: slink push")
	os.Exit(exitCode)
}
