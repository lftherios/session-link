//go:build !windows

package main

import (
	"bufio"
	"bytes"
	"io"
	"net"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/lftherios/session-link/internal/importers"
)

func TestViewSourcesNeverSubstitutesExplicitIdentity(t *testing.T) {
	root := t.TempDir()
	loc := importers.Locations{Pi: filepath.Join(root, "pi"), Claude: filepath.Join(root, "claude")}
	for file, data := range map[string]string{
		filepath.Join(loc.Pi, "--work-project--", "latest.jsonl"): `{"type":"session","id":"same"}`,
		filepath.Join(loc.Claude, "-work-project", "same.jsonl"):  `{"type":"user"}`,
	} {
		if err := os.MkdirAll(filepath.Dir(file), 0o700); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(file, []byte(data), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	captures := filepath.Join(root, "captures")
	if _, err := viewSources(loc, "/work/project", "", "missing", true, captures); err == nil || !strings.Contains(err.Error(), "no session") {
		t.Fatalf("missing ID: %v", err)
	}
	if _, err := viewSources(loc, "/work/project", "", "same", true, captures); err == nil || !strings.Contains(err.Error(), "ambiguous") {
		t.Fatalf("ambiguous ID: %v", err)
	}
	selected, err := viewSources(loc, "/work/project/src", "pi", "same", true, captures)
	if err != nil || len(selected) != 1 || selected[0].Harness != "pi" {
		t.Fatalf("selected: %+v %v", selected, err)
	}
	if _, err := viewSources(loc, "/work/project", "typo", "", false, captures); err == nil {
		t.Fatal("unknown harness accepted")
	}
}

func stopView(t *testing.T, address string) {
	t.Helper()
	base := address[:strings.Index(address[7:], "/")+7]
	req, _ := http.NewRequest("POST", base+"/api/stop", nil)
	req.Header.Set("x-slink", "1")
	client := &http.Client{Timeout: 3 * time.Second}
	resp, err := client.Do(req)
	if err != nil {
		t.Errorf("stop: %v", err)
		return
	}
	resp.Body.Close()
	if resp.StatusCode != 200 {
		t.Errorf("stop: %d", resp.StatusCode)
	}
}

func TestViewCLIBackgroundAndForeground(t *testing.T) {
	bin := buildSlink(t)
	fixture, err := filepath.Abs("../../../testdata/import/pi/basic/input.session.jsonl")
	if err != nil {
		t.Fatal(err)
	}
	for _, name := range []string{"foreground", "background", "browser-failure"} {
		background := name == "background"
		t.Run(name, func(t *testing.T) {
			args := []string{"view", "--session", fixture}
			if name != "browser-failure" {
				args = append(args, "--no-browser")
			}
			if background {
				args = append(args, "--background")
			}
			cmd := exec.Command(bin, args...)
			cmd.Env = append(os.Environ(), "SLINK_HOME="+t.TempDir(), "SSH_CONNECTION=fixture", "SLINK_VIEW_CHILD=")
			if name == "browser-failure" {
				cmd.Env = append(cmd.Env, "PATH="+t.TempDir(), "SSH_CONNECTION=", "SSH_TTY=")
			}
			var stderr bytes.Buffer
			cmd.Stderr = &stderr
			stdout, err := cmd.StdoutPipe()
			if err != nil {
				t.Fatal(err)
			}
			if err := cmd.Start(); err != nil {
				t.Fatal(err)
			}
			defer cmd.Process.Kill()
			ready := make(chan string, 1)
			go func() { line, _ := bufio.NewReader(stdout).ReadString('\n'); ready <- strings.TrimSpace(line) }()
			var address string
			select {
			case address = <-ready:
			case <-time.After(10 * time.Second):
				t.Fatal("no ready URL")
			}
			if !strings.HasPrefix(address, "http://127.0.0.1:") || !strings.Contains(address, "/p/") {
				t.Fatalf("bad URL %q", address)
			}
			stopped := false
			defer func() {
				if !stopped {
					stopView(t, address)
				}
			}()
			client := &http.Client{Timeout: 3 * time.Second}
			resp, err := client.Get(address)
			if err != nil {
				t.Fatal(err)
			}
			body, _ := io.ReadAll(resp.Body)
			resp.Body.Close()
			if resp.StatusCode != 200 || !bytes.Contains(body, []byte("notes.txt")) {
				t.Fatalf("page: %d %s", resp.StatusCode, body)
			}
			done := make(chan error, 1)
			go func() { done <- cmd.Wait() }()
			if !background {
				stopView(t, address)
				stopped = true
			}
			select {
			case err := <-done:
				if err != nil {
					t.Fatalf("exit: %v %s", err, stderr.String())
				}
			case <-time.After(5 * time.Second):
				t.Fatal("viewer did not release invoking process")
			}
			expected := "ssh -N -L"
			if name == "browser-failure" {
				expected = "Could not open a browser"
			}
			if !strings.Contains(stderr.String(), expected) || strings.Contains(stderr.String(), "PID -1") {
				t.Fatal(stderr.String())
			}
			if background {
				stopView(t, address)
				stopped = true
			}
		})
	}
	// A port conflict must report recovery and must not print a ready URL.
	listener, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer listener.Close()
	_, port, _ := net.SplitHostPort(listener.Addr().String())
	cmd := exec.Command(bin, "view", "--session", fixture, "--no-browser", "--port", port)
	cmd.Env = append(os.Environ(), "SLINK_HOME="+t.TempDir())
	var stdout, stderr bytes.Buffer
	cmd.Stdout, cmd.Stderr = &stdout, &stderr
	if err := cmd.Run(); err == nil || stdout.Len() != 0 || !strings.Contains(stderr.String(), "--port 0") {
		t.Fatalf("conflict: %v %s %s", err, stdout.String(), stderr.String())
	}
}
