package main

import (
	"encoding/json"
	"flag"
	"fmt"
	"net/http"
	"net/url"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"golang.org/x/term"
)

/* ---------------------------------------------------- tap port/pid files */

func tapPortFile() string { return filepath.Join(cli.Home(), "tap.port") }
func tapPidFile() string  { return filepath.Join(cli.Home(), "tap.pid") }

// writeTapFiles records where the tap listens and under which pid, so
// `on`, `status`, and `tap --stop` can find it later.
func writeTapFiles(port int) {
	os.MkdirAll(cli.Home(), 0o755)
	os.WriteFile(tapPortFile(), []byte(strconv.Itoa(port)+"\n"), 0o644)
	os.WriteFile(tapPidFile(), []byte(strconv.Itoa(os.Getpid())+"\n"), 0o644)
}

// removeTapFiles cleans up only this process's own registration — a
// starter that lost the bind race must never erase the files of the tap
// that is actually running.
func removeTapFiles() {
	b, err := os.ReadFile(tapPidFile())
	if err == nil {
		if pid, _ := strconv.Atoi(strings.TrimSpace(string(b))); pid != os.Getpid() {
			return
		}
	}
	os.Remove(tapPortFile())
	os.Remove(tapPidFile())
}

// persistedTapPort is the port the last tap recorded, or 0.
func persistedTapPort() int {
	b, err := os.ReadFile(tapPortFile())
	if err != nil {
		return 0
	}
	p, _ := strconv.Atoi(strings.TrimSpace(string(b)))
	return p
}

// tapInfo queries a tap's health endpoint; ok is false for anything that
// isn't a slink tap (a stray HTTP server on the port must not be trusted
// with LLM traffic).
type tapStatus struct {
	Service   string            `json:"service"`
	Sessions  int               `json:"sessions"`
	Upstreams map[string]string `json:"upstreams"`
}

func tapInfo(port int) (*tapStatus, bool) {
	client := &http.Client{Timeout: 500 * time.Millisecond}
	res, err := client.Get(fmt.Sprintf("http://127.0.0.1:%d/", port))
	if err != nil {
		return nil, false
	}
	defer res.Body.Close()
	var st tapStatus
	if res.StatusCode != 200 || json.NewDecoder(res.Body).Decode(&st) != nil {
		return nil, false
	}
	if st.Service != "session.link" {
		return nil, false
	}
	return &st, true
}

/* ---------------------------------------------------------------- status */

func runStatus(args []string) {
	fs := flag.NewFlagSet("status", flag.ExitOnError)
	asJSON := fs.Bool("json", false, "emit JSON")
	setUsage(fs, "slink status",
		"One honest screen: recorder, this shell's routing, upstreams,\n  account, and what's been captured.",
		"slink status")
	parseReordered(fs, args)

	port := persistedTapPort()
	if port == 0 {
		port = 4141
	}
	info, up := tapInfo(port)

	routed, routedLive := shellRouting()
	captures := cli.ListCaptures(cli.CaptureDir())
	cfg := cli.ReadConfig()
	target, key := cli.ResolveTarget("", "")

	if *asJSON {
		out := map[string]any{
			"recorder": map[string]any{"running": up, "port": port},
			"routing":  map[string]any{"this_shell": routed, "tap_reachable": routedLive},
			"account":  map[string]any{"signed_in": key != "", "login": cfg.Login, "server": target},
			"sessions": map[string]any{"count": len(captures)},
		}
		if up {
			out["recorder"].(map[string]any)["open_sessions"] = info.Sessions
			out["recorder"].(map[string]any)["upstreams"] = info.Upstreams
		}
		if len(captures) > 0 {
			out["sessions"].(map[string]any)["latest_at"] = captures[0].CreatedAt
			out["sessions"].(map[string]any)["latest_id"] = captureID(captures[0].File)
		}
		json.NewEncoder(os.Stdout).Encode(out)
		return
	}

	row := func(k, v string) { fmt.Printf("%-10s %s\n", k, v) }
	if up {
		state := fmt.Sprintf("running on 127.0.0.1:%d", port)
		if info.Sessions > 0 {
			state += fmt.Sprintf(" · %s open", plural(info.Sessions, "session"))
		}
		row("Recorder", state)
	} else {
		row("Recorder", "not running — start it with: slink tap   (or: slink setup)")
	}
	switch {
	case routed && routedLive:
		row("Routing", "on in this shell")
	case routed:
		row("Routing", "BROKEN — this shell routes to a tap that isn't running")
		fmt.Printf("%-10s agents will see: connection refused\n", "")
		fmt.Printf("%-10s fix: slink tap   (or: eval \"$(slink off)\")\n", "")
	default:
		row("Routing", `off in this shell — eval "$(slink on)"`)
	}
	if up {
		for _, prov := range []string{"anthropic", "openai"} {
			u := info.Upstreams[prov]
			label := strings.ToUpper(prov[:1]) + prov[1:]
			switch u {
			case "https://api.anthropic.com", "https://api.openai.com":
				row(label, "default upstream")
			default:
				row(label, "proxying "+u)
			}
		}
	}
	switch {
	case cfg.Login != "":
		row("Account", "@"+cfg.Login+" → "+target)
	case key != "":
		row("Account", "signed in (API key) → "+target)
	default:
		row("Account", "not signed in — capture works without one; publishing needs: slink login")
	}
	if len(captures) == 0 {
		row("Sessions", "none yet")
	} else {
		row("Sessions", fmt.Sprintf("%d · latest %s (%s)", len(captures), ago(captures[0].CreatedAt), captureID(captures[0].File)))
	}
}

// shellRouting reports whether this shell's BASE_URLs point at a tap, and
// whether that tap is actually answering — "routed to a dead port" is a
// broken state, not an on state.
func shellRouting() (routed, live bool) {
	live = true
	for _, env := range []string{"ANTHROPIC_BASE_URL", "OPENAI_BASE_URL"} {
		if !tapShaped(env) {
			continue
		}
		routed = true
		if u, err := url.Parse(os.Getenv(env)); err == nil {
			if p, err := strconv.Atoi(u.Port()); err == nil {
				if _, up := tapInfo(p); !up {
					live = false
				}
			}
		}
	}
	return routed, routed && live
}

/* ------------------------------------------------------------- tap stop */

// stopTap terminates the tap daemon. Safety rule: a signal is only ever
// sent when a live slink tap is confirmed on the recorded port — a pid
// alone proves nothing (pids get reused; SIGTERMing a stranger's process
// is not an acceptable failure mode).
func stopTap() {
	port := persistedTapPort()
	if port == 0 {
		port = 4141
	}
	_, tapUp := tapInfo(port)
	pidB, pidErr := os.ReadFile(tapPidFile())

	if !tapUp {
		if pidErr == nil {
			os.Remove(tapPortFile())
			os.Remove(tapPidFile())
			fmt.Fprintln(os.Stderr, "no tap is running (cleaned up stale files)")
		} else {
			fmt.Fprintln(os.Stderr, "no tap is running")
		}
		return
	}
	if pidErr != nil {
		die(fmt.Sprintf("✗ a tap is listening on :%d but left no pidfile (older slink?)\n\n  stop it manually:  pkill -f \"slink tap\"", port))
	}
	pid, _ := strconv.Atoi(strings.TrimSpace(string(pidB)))
	if pid <= 1 {
		die("corrupt pidfile " + tapPidFile())
	}
	proc, err := os.FindProcess(pid)
	if err != nil {
		die(fmt.Sprintf("cannot signal pid %d: %v", pid, err))
	}
	proc.Signal(syscall.SIGTERM)
	stopped := false
	for deadline := time.Now().Add(8 * time.Second); time.Now().Before(deadline); {
		if _, up := tapInfo(port); !up {
			stopped = true
			break
		}
		time.Sleep(100 * time.Millisecond)
	}
	if !stopped {
		die(fmt.Sprintf("tap (pid %d) didn't stop within 8s — it may be mid-write; try again or: kill %d", pid, pid))
	}
	fmt.Fprintf(os.Stderr, "✓ tap stopped (was pid %d on :%d) — open sessions were finalized\n", pid, port)
	// A login service with keep-alive will bring it right back — say so
	// instead of letting the next `status` contradict this success line.
	if cli.ServiceInstalled() {
		fmt.Fprintln(os.Stderr, "  note: the tap is installed as a login service and may restart — remove it with: slink tap --uninstall")
	}
}

/* ---------------------------------------------------------------- delete */

func runDelete(args []string) {
	fs := flag.NewFlagSet("delete", flag.ExitOnError)
	yes := fs.Bool("yes", false, "delete without confirmation")
	server := fs.String("server", "", "override the server")
	key := fs.String("key", "", "override the API key")
	setUsage(fs, "slink delete <url | published-id>",
		"Take a published session offline. The link answers 410 immediately.\n  Only your own publishes can be deleted.",
		"slink delete https://session.link/r/9f3kx2mvq7wtd4")
	parseReordered(fs, args)

	ref := fs.Arg(0)
	if ref == "" {
		die("which one? pass the published URL or its id: slink delete <url|id>")
	}
	// Browser-copied URLs carry query strings, fragments, trailing slashes.
	id := ref
	for _, sep := range []string{"?", "#"} {
		if i := strings.Index(id, sep); i >= 0 {
			id = id[:i]
		}
	}
	if i := strings.LastIndex(id, "/r/"); i >= 0 {
		id = id[i+3:]
	}
	id = strings.TrimSuffix(id, "/")
	if id == "" || strings.ContainsAny(id, "/?#") {
		die(fmt.Sprintf("%q doesn't look like a published URL or id", ref))
	}

	target, apiKey := cli.ResolveTarget(*server, *key)
	if apiKey == "" {
		die("not signed in — run: slink login  (deleting needs the account that published it)")
	}
	if !*yes {
		// term.IsTerminal, not a char-device stat: /dev/null is a char
		// device, and a script must never hang on an invisible prompt.
		if !term.IsTerminal(int(os.Stdin.Fd())) {
			die("not a TTY — pass --yes to delete from scripts")
		}
		fmt.Fprintf(os.Stderr, "about to take %s/r/%s offline\n", target, id)
		fmt.Fprintln(os.Stderr, "  the link stops working immediately")
		fmt.Fprint(os.Stderr, "\nTake it offline? [y/N] ")
		var line string
		fmt.Fscanln(os.Stdin, &line)
		if l := strings.ToLower(strings.TrimSpace(line)); l != "y" && l != "yes" {
			fmt.Fprintln(os.Stderr, "aborted — still online")
			os.Exit(1)
		}
	}
	status, body := cli.DeleteRun(target, apiKey, id)
	switch {
	case status >= 200 && status < 300 || status == 410:
		fmt.Fprintln(os.Stderr, "✓ taken offline — the page now shows a deletion notice")
		fmt.Fprintf(os.Stderr, "    url:  %s/r/%s\n", target, id)
		fmt.Fprintln(os.Stderr, "\n  note: publishing the identical session again would revive this URL")
	case status == 0:
		die(fmt.Sprintf("✗ can't reach %s (%s)\n    nothing was deleted", target, transportCause(body)))
	case status == 401:
		die("✗ not signed in — run: slink login")
	case status == 403:
		die("✗ that session was published by a different account")
	case status == 404:
		die("✗ no published session " + id + " on " + target)
	default:
		die(fmt.Sprintf("✗ delete failed (HTTP %d): %s", status, body))
	}
}

/* ---------------------------------------------------------------- logout */

func runLogout(args []string) {
	fs := flag.NewFlagSet("logout", flag.ExitOnError)
	setUsage(fs, "slink logout",
		"Forget the saved API key. Capturing is unaffected; published\n  sessions stay up (delete them individually with slink delete).",
		"slink logout")
	parseReordered(fs, args)
	c := cli.ReadConfig()
	if c.APIKey == "" {
		fmt.Fprintln(os.Stderr, "not signed in")
		return
	}
	was := c.Login
	c.APIKey, c.Login = "", ""
	if _, err := cli.WriteConfig(c); err != nil {
		die(err.Error())
	}
	if was != "" {
		fmt.Fprintf(os.Stderr, "✓ signed out @%s — the key was removed from this machine\n", was)
	} else {
		fmt.Fprintln(os.Stderr, "✓ signed out — the key was removed from this machine")
	}
}
