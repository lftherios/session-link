package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"net"
	"net/http"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/open"
	"github.com/lftherios/session-link/internal/tap"
)

func runOpen(args []string) {
	fs := flag.NewFlagSet("view", flag.ExitOnError)
	port := fs.Int("port", 4400, "listen port")
	noBrowser := fs.Bool("no-browser", false, "don't open the browser")
	setUsage(fs, "slink view [flags] [session-id]",
		"Browse local sessions at 127.0.0.1 in the same viewer the hosted site\n  uses. With an id, opens that session. The Publish button runs the same\n  validate + secret-scan gate as push.",
		"slink view 2a9a48")
	parseReordered(fs, args)

	// `slink view <id>` — the instruction import prints — must actually
	// land on that session, and an unknown id must say so, not silently
	// serve the index.
	focus := ""
	if ref := fs.Arg(0); ref != "" {
		file, err := resolveCapture(ref)
		if err != nil {
			die(err.Error())
		}
		focus = "/r/" + strings.TrimSuffix(filepath.Base(file), ".json")
	}

	target, apiKey := cli.ResolveTarget("", "")
	srv := &open.Server{CaptureDir: cli.CaptureDir(), Target: target, APIKey: apiKey}
	addr, _, err := srv.Serve(*port)
	if err != nil {
		if strings.Contains(err.Error(), "address already in use") {
			die(fmt.Sprintf("✗ can't listen on 127.0.0.1:%d — that port is in use\n\n  another viewer may already be open — reuse it, or: slink view --port <port>", *port))
		}
		die(err.Error())
	}
	fmt.Fprintln(os.Stderr, "local viewer running")
	if focus != "" {
		fmt.Fprintf(os.Stderr, "    session:    %s%s\n", addr, focus)
	} else {
		fmt.Fprintf(os.Stderr, "    browse:     %s\n", addr)
	}
	fmt.Fprintf(os.Stderr, "    publishes:  %s\n", target)
	fmt.Fprintln(os.Stderr, "\n  Ctrl-C to stop")
	if !*noBrowser {
		cli.OpenBrowser(addr + focus)
	}
	select {} // serve until interrupted
}

// runDev is wrapper mode: record one session around a command (or until
// Ctrl-C without one), then finalize and summarize.
func runDev(args []string) {
	fs := flag.NewFlagSet("record", flag.ExitOnError)
	port := fs.Int("port", -1, "listen port (default: ephemeral when wrapping, 4141 otherwise)")
	name := fs.String("name", "", "session name")
	setUsage(fs, "slink record [flags] -- <command …>   (alias: dev)",
		"Run a command with its LLM calls recorded to a local session. Without\n  a command, prints proxy exports for this shell instead (Ctrl-C to finish).",
		"slink record -- python agent.py")

	// Everything after "--" is the child command, verbatim — only slink's
	// own flags are reordered. Without "--", legacy `slink dev cmd …` still
	// parses (flags first, command as the trailing positionals).
	var cmdArgs []string
	dashdash := -1
	for i, a := range args {
		if a == "--" {
			dashdash = i
			break
		}
	}
	if dashdash >= 0 {
		parseReordered(fs, args[:dashdash])
		if stray := fs.Args(); len(stray) > 0 {
			die(fmt.Sprintf("unexpected argument before --: %q\n  the command to record goes after --: slink record [flags] -- <command …>", stray[0]))
		}
		cmdArgs = args[dashdash+1:]
	} else {
		fs.Parse(args)
		cmdArgs = fs.Args()
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
		if strings.Contains(err.Error(), "address already in use") {
			die(fmt.Sprintf("✗ can't listen on 127.0.0.1:%d — that port is in use\n\n  pick another:  slink record --port <port> -- <cmd>\n  or drop --port to use a random free one", listenPort))
		}
		die(err.Error())
	}
	httpSrv := &http.Server{Handler: srv}
	go httpSrv.Serve(ln)
	p := ln.Addr().(*net.TCPAddr).Port
	env := []string{
		fmt.Sprintf("ANTHROPIC_BASE_URL=http://127.0.0.1:%d/anthropic", p),
		fmt.Sprintf("OPENAI_BASE_URL=http://127.0.0.1:%d/openai/v1", p),
	}
	fmt.Fprintln(os.Stderr, "recording")
	fmt.Fprintf(os.Stderr, "    proxy:  http://127.0.0.1:%d\n", p)
	fmt.Fprintf(os.Stderr, "    saves:  %s\n", displayPath(session.File))

	exitCode := 0
	startFailed := false
	if len(cmdArgs) > 0 {
		child := exec.Command(cmdArgs[0], cmdArgs[1:]...)
		child.Stdin, child.Stdout, child.Stderr = os.Stdin, os.Stdout, os.Stderr
		child.Env = append(os.Environ(), env...)
		// Stay alive through Ctrl-C so the capture always finalizes — but
		// catch the signal instead of ignoring it: an ignored SIGINT is
		// inherited across exec, which made every wrapped agent
		// un-interruptible. Signals are forwarded to the child (a signal
		// aimed at slink alone must still stop the run), and the handler
		// stays registered through Finalize below so an impatient second
		// Ctrl-C can't kill the capture mid-assembly.
		sigc := make(chan os.Signal, 8)
		signal.Notify(sigc, os.Interrupt, syscall.SIGTERM)
		defer signal.Stop(sigc)
		if err := child.Start(); err != nil {
			startFailed = true
			if errors.Is(err, exec.ErrNotFound) {
				fmt.Fprintf(os.Stderr, "✗ could not start %s — not found in PATH\n", cmdArgs[0])
				exitCode = 127
			} else {
				fmt.Fprintf(os.Stderr, "✗ could not start %s — %v\n", cmdArgs[0], err)
				exitCode = 126
			}
		} else {
			done := make(chan struct{})
			go func() {
				for {
					select {
					case s := <-sigc:
						child.Process.Signal(s)
					case <-done:
						// Keep draining so late signals never kill the
						// wrapper before the capture is finalized.
						for {
							select {
							case <-sigc:
							case <-time.After(time.Minute):
								return
							}
						}
					}
				}
			}()
			werr := child.Wait()
			close(done)
			if ee, ok := werr.(*exec.ExitError); ok {
				exitCode = exitStatus(ee)
			} else if werr != nil {
				exitCode = 1
			}
		}
	} else {
		for _, e := range env {
			fmt.Printf("export %s\n", e)
		}
		fmt.Fprintln(os.Stderr, "Ctrl-C to finish")
		// Stay registered (and keep draining) through Finalize below — a
		// second Ctrl-C must not kill the capture mid-assembly.
		sigc := make(chan os.Signal, 8)
		signal.Notify(sigc, os.Interrupt, syscall.SIGTERM)
		<-sigc
		go func() {
			for range sigc {
			}
		}()
		fmt.Fprintln(os.Stderr)
	}

	shutCtx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	httpSrv.Shutdown(shutCtx)
	cancel()
	httpSrv.Close()
	n, ferr := session.Finalize()
	switch {
	case n == 0 && startFailed:
		fmt.Fprintln(os.Stderr, "  nothing was recorded")
	case n == 0:
		fmt.Fprintln(os.Stderr, "captured 0 LLM calls — nothing was saved")
		fmt.Fprintln(os.Stderr, "  if your tool called a model, it may not honor ANTHROPIC_BASE_URL / OPENAI_BASE_URL")
	case ferr != nil:
		fmt.Fprintf(os.Stderr, "✗ recorded %s but couldn't finalize: %v\n", plural(n, "call"), ferr)
		fmt.Fprintf(os.Stderr, "  the partial spool is kept at %s.spool\n", session.File)
	default:
		id := captureID(session.File)
		fmt.Fprintf(os.Stderr, "✓ captured %s\n", plural(n, "call"))
		fmt.Fprintf(os.Stderr, "    stored:  %s\n", displayPath(session.File))
		fmt.Fprintf(os.Stderr, "\n  review:  slink view %s\n  publish: slink share %s\n", id, id)
	}
	os.Exit(exitCode)
}

// exitStatus maps a child's death to the shell convention: its own exit
// code, or 128+signal when a signal killed it.
func exitStatus(ee *exec.ExitError) int {
	if ws, ok := ee.Sys().(syscall.WaitStatus); ok && ws.Signaled() {
		return 128 + int(ws.Signal())
	}
	return ee.ExitCode()
}
