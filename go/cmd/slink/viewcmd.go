package main

import (
	"bufio"
	"encoding/json"
	"flag"
	"fmt"
	"io"
	"net/url"
	"os"
	"os/exec"
	"os/signal"
	"path/filepath"
	"sort"
	"strings"
	"sync"
	"syscall"
	"time"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/handoff"
	"github.com/lftherios/session-link/internal/importers"
	"github.com/lftherios/session-link/internal/open"
	"github.com/lftherios/session-link/internal/spool"
)

func runOpen(args []string) {
	fs := flag.NewFlagSet("view", flag.ExitOnError)
	port := fs.Int("port", 0, "listen port (default: a free port)")
	noBrowser := fs.Bool("no-browser", false, "print the local URL without opening a browser")
	background := fs.Bool("background", false, "return to the harness while the local viewer stays running")
	from := fs.String("from", "", "agent: claude-code|codex|pi|opencode|hermes")
	session := fs.String("session", "", "explicit harness session ID or transcript path")
	span := fs.String("span", "", "start at this span in the viewer")
	pick := fs.Bool("pick", false, "always show the browser session picker")
	setUsage(fs, "slink view [flags] [session-id | transcript | session.json]",
		"Open agent work in a local web preview, without signing in or uploading.\n  Finds sessions for this project across harnesses. Multiple sessions open\n  a browser picker; --session identifies the exact session to preview.\n  The preview is saved separately, so later agent work cannot change it.",
		"slink view\n  slink view --from pi --session <transcript> --background\n  slink view --no-browser --port 4400")
	parseReordered(fs, args)
	if fs.NArg() > 1 || (*session != "" && fs.NArg() > 0) {
		die("pass one session reference, either as an argument or with --session")
	}
	if *pick && (*session != "" || fs.NArg() > 0) {
		die("--pick cannot be combined with an explicit session")
	}
	if *port < 0 || *port > 65535 {
		die("--port must be between 0 and 65535")
	}
	if *span != "" && !validSpanID(*span) {
		die("--span must be a span ID (1–64 letters, digits, dots, underscores, or hyphens)")
	}
	if *background && os.Getenv("SLINK_VIEW_CHILD") != "1" {
		url, err := backgroundView(args)
		if err != nil {
			die(err.Error())
		}
		fmt.Println(url)
		openViewBrowser(url, *noBrowser)
		return
	}
	cwd, err := os.Getwd()
	if err != nil {
		die(err.Error())
	}
	ref := firstNonEmptyStr(*session, fs.Arg(0))
	sources, err := viewSources(importers.DefaultLocations(), cwd, *from, ref, *session != "", cli.CaptureDir())
	if err != nil && (ref != "" || *from != "") {
		die(err.Error())
	}
	warning := ""
	if err != nil {
		warning = err.Error()
		fmt.Fprintf(os.Stderr, "Some sessions could not be listed: %s\n", warning)
	}
	previewDir := filepath.Join(cli.Home(), "previews")
	focus := ""
	if ref != "" || (!*pick && warning == "" && len(sources) == 1 && sources[0].Dir != "") {
		id, err := handoff.Save(previewDir, sources[0])
		if err != nil {
			die(err.Error())
		}
		focus = "/p/" + id
	}
	if *span != "" && focus == "" {
		die("--span needs an explicit session when several sessions are available")
	}
	if *span != "" {
		focus += "#span=" + *span
	}
	target, key := cli.ResolveTarget("", "")
	stop := make(chan struct{})
	var once sync.Once
	srv := &open.Server{CaptureDir: cli.CaptureDir(), PreviewDir: previewDir, Target: target, APIKey: key, Project: cwd, Sources: sources, Warning: warning,
		OnStop:    func() { once.Do(func() { close(stop) }) },
		OnPublish: func(url string) { fmt.Fprintf(os.Stderr, "Published: %s\n", url) },
	}
	addr, closeServer, err := srv.Serve(*port)
	if err != nil {
		die(fmt.Sprintf("cannot start the local viewer: %v\n  try slink view --port 0 to use a free port", err))
	}
	defer closeServer()
	url := addr + focus
	fmt.Println(url) // Machine-readable handoff; diagnostics stay on stderr.
	child := os.Getenv("SLINK_VIEW_CHILD") == "1"
	if child {
		os.Stdout.Close()
	} // Release the parent's readiness pipe.
	fmt.Fprintf(os.Stderr, "Local preview ready · nothing uploaded\n  %s\n  Saved previews: %s\n", url, displayPath(previewDir))
	if child {
		fmt.Fprintln(os.Stderr, "Stop the viewer from the sessions page.")
	} else {
		fmt.Fprintln(os.Stderr, "Ctrl-C to stop the viewer.")
	}
	if !child {
		openViewBrowser(url, *noBrowser)
	}
	sigc := make(chan os.Signal, 1)
	signal.Notify(sigc, os.Interrupt, syscall.SIGTERM)
	defer signal.Stop(sigc)
	select {
	case <-sigc:
	case <-stop:
	}
}

func validSpanID(id string) bool {
	if len(id) < 1 || len(id) > 64 {
		return false
	}
	for _, c := range id {
		if !((c >= 'a' && c <= 'z') || (c >= 'A' && c <= 'Z') || (c >= '0' && c <= '9') || c == '.' || c == '_' || c == '-') {
			return false
		}
	}
	return true
}

func openViewBrowser(address string, disabled bool) {
	if os.Getenv("SSH_CONNECTION") != "" || os.Getenv("SSH_TTY") != "" {
		parsed, _ := url.Parse(address)
		port := parsed.Port()
		fmt.Fprintf(os.Stderr, "Remote shell: forward this port from your own computer:\n  ssh -N -L %s:127.0.0.1:%s <this-host>\nThen open the printed URL locally.\n", port, port)
		return
	}
	if disabled {
		return
	}
	if err := cli.OpenBrowser(address); err != nil {
		fmt.Fprintf(os.Stderr, "Could not open a browser: %v\nOpen the printed URL manually; the preview is ready.\n", err)
	}
}

func viewSources(loc importers.Locations, cwd, from, ref string, explicitNative bool, capturesDir string) ([]handoff.Source, error) {
	if from != "" && importers.Registry[from] == nil {
		return nil, fmt.Errorf("unknown agent %q — known: %s", from, strings.Join(knownHarnesses(), ", "))
	}
	if ref != "" {
		if fileExists(ref) || fileExists(spool.SpoolPath(ref)) {
			source, err := handoff.File(ref, from)
			return []handoff.Source{source}, err
		}
		if !explicitNative && from == "" {
			var matches []handoff.Source
			for _, c := range cli.ListCaptures(capturesDir) {
				if captureID(c.File) == ref {
					source, err := handoff.File(c.File, "")
					if err != nil {
						return nil, err
					}
					matches = append(matches, source)
				}
			}
			if len(matches) == 1 {
				return matches, nil
			}
			if len(matches) > 1 {
				return nil, fmt.Errorf("session ID %q matches multiple captures; pass a full path", ref)
			}
		}
		candidates, err := loc.Recent(from, cwd, ref, 30)
		if err != nil {
			return nil, err
		}
		if len(candidates) == 0 {
			return nil, fmt.Errorf("no session %q for this project; pass its transcript path, or use slink view --pick", ref)
		}
		if len(candidates) > 1 {
			return nil, fmt.Errorf("session ID %q is ambiguous; add --from or pass a transcript path", ref)
		}
		return []handoff.Source{handoff.Native(candidates[0])}, nil
	}
	candidates, err := loc.Recent(from, cwd, "", 30)
	if err != nil && from != "" {
		return nil, err
	}
	sources := make([]handoff.Source, 0, len(candidates))
	for _, c := range candidates {
		sources = append(sources, handoff.Native(c))
	}
	if from != "" {
		return sources, nil
	}
	for _, capture := range cli.ListCaptures(capturesDir) {
		data, err := os.ReadFile(capture.File)
		if err != nil {
			continue
		}
		var doc struct {
			Metadata struct {
				Cwd string `json:"cwd"`
			} `json:"metadata"`
		}
		json.Unmarshal(data, &doc)
		dir := doc.Metadata.Cwd
		if dir != "" && filepath.Clean(cwd) != filepath.Clean(dir) && !strings.HasPrefix(filepath.Clean(cwd), filepath.Clean(dir)+string(os.PathSeparator)) {
			continue
		}
		source, err := handoff.File(capture.File, "")
		if err != nil {
			continue
		}
		source.Title, source.Dir = capture.Name, dir
		if source.Title == "" {
			source.Title = source.ID
		}
		sources = append(sources, source)
	}
	sort.SliceStable(sources, func(i, j int) bool { return sources[i].Updated.After(sources[j].Updated) })
	return sources, err
}

// backgroundView waits for a real listening URL before releasing the child.
// Its stderr goes to a log, so a harness can await the short parent process.
func backgroundView(args []string) (string, error) {
	exe, err := os.Executable()
	if err != nil {
		return "", err
	}
	if err := os.MkdirAll(cli.Home(), 0o700); err != nil {
		return "", err
	}
	log, err := os.CreateTemp(cli.Home(), "viewer-*.log")
	if err != nil {
		return "", err
	}
	defer log.Close()
	cmd := exec.Command(exe, append([]string{"view"}, args...)...)
	cmd.Env = append(os.Environ(), "SLINK_VIEW_CHILD=1")
	cmd.Stderr = log
	out, err := cmd.StdoutPipe()
	if err != nil {
		return "", err
	}
	detach(cmd)
	if err := cmd.Start(); err != nil {
		return "", err
	}
	ready := make(chan string, 1)
	go func() {
		line, _ := bufio.NewReader(io.LimitReader(out, 4096)).ReadString('\n')
		ready <- strings.TrimSpace(line)
	}()
	var url string
	select {
	case url = <-ready:
	case <-time.After(30 * time.Second):
	}
	if !strings.HasPrefix(url, "http://127.0.0.1:") {
		cmd.Process.Kill()
		cmd.Wait()
		data, _ := os.ReadFile(log.Name())
		return "", fmt.Errorf("could not start the background viewer: %s\n  details: %s", strings.TrimSpace(string(data)), log.Name())
	}
	out.Close()
	pid := cmd.Process.Pid
	cmd.Process.Release()
	fmt.Fprintf(os.Stderr, "Local viewer running (PID %d). Stop it from the sessions page.\nLog: %s\n", pid, displayPath(log.Name()))
	return url, nil
}
