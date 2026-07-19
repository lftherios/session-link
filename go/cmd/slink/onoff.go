package main

import (
	"flag"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"time"

	"github.com/lftherios/session-link/internal/cli"
)

func tapReachable(port int) bool {
	client := &http.Client{Timeout: 500 * time.Millisecond}
	res, err := client.Get(fmt.Sprintf("http://127.0.0.1:%d/", port))
	if err != nil {
		return false
	}
	res.Body.Close()
	return res.StatusCode == 200
}

// startTapDaemon starts the tap detached so it outlives this invocation
// (and the shell that ran it), logging to ~/.slink/tap.log.
func startTapDaemon(port int) error {
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
	cmd.Stdout = out
	cmd.Stderr = out
	detach(cmd)
	if err := cmd.Start(); err != nil {
		return err
	}
	return cmd.Process.Release()
}

// runOn: `eval "$(slink on)"` — only export lines go to stdout so eval
// stays clean; hints go to stderr.
func runOn(args []string) {
	fs := flag.NewFlagSet("on", flag.ExitOnError)
	port := fs.Int("port", 4141, "tap port")
	noStart := fs.Bool("no-start", false, "don't auto-start the tap")
	fs.Parse(args)
	if !tapReachable(*port) {
		if *noStart {
			fmt.Fprintf(os.Stderr, "# tap not running on :%d — start it with: slink tap\n", *port)
		} else if err := startTapDaemon(*port); err != nil {
			fmt.Fprintf(os.Stderr, "# could not start the tap: %v\n", err)
		} else {
			up := false
			for deadline := time.Now().Add(2500 * time.Millisecond); time.Now().Before(deadline); {
				if tapReachable(*port) {
					up = true
					break
				}
				time.Sleep(100 * time.Millisecond)
			}
			if up {
				fmt.Fprintf(os.Stderr, "# started the tap on :%d\n", *port)
			} else {
				fmt.Fprintf(os.Stderr, "# tap didn't come up on :%d — check %s\n", *port, cli.TapLogPath())
			}
		}
	}
	fmt.Printf("export ANTHROPIC_BASE_URL=http://127.0.0.1:%d/anthropic\n", *port)
	fmt.Printf("export OPENAI_BASE_URL=http://127.0.0.1:%d/openai/v1\n", *port)
	fmt.Fprintln(os.Stderr, "# capture is on for this shell — publish a session later with: slink push")
}

func runOff() {
	fmt.Println("unset ANTHROPIC_BASE_URL")
	fmt.Println("unset OPENAI_BASE_URL")
	fmt.Fprintln(os.Stderr, "# capture off for this shell")
}

func runLogin(args []string) {
	fs := flag.NewFlagSet("login", flag.ExitOnError)
	key := fs.String("key", "", "paste an rk_… API key instead of the browser flow")
	server := fs.String("server", "", "login against a specific server")
	fs.Parse(args)

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
