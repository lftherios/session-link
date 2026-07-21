//go:build !windows

package main

import (
	"bytes"
	"os/exec"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
)

// buildSlink compiles the CLI once per test binary into a temp dir.
func buildSlink(t *testing.T) string {
	t.Helper()
	bin := filepath.Join(t.TempDir(), "slink")
	cmd := exec.Command("go", "build", "-o", bin, ".")
	if out, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("go build: %v\n%s", err, out)
	}
	return bin
}

// startWrapped launches `slink dev -- sleep 30` in its own process group
// with a sandboxed SLINK_HOME, waiting for the recording banner so the
// child is known to be running.
func startWrapped(t *testing.T, bin string) (*exec.Cmd, *bytes.Buffer, chan error) {
	t.Helper()
	var stderr bytes.Buffer
	cmd := exec.Command(bin, "dev", "--", "sleep", "30")
	cmd.Stderr = &stderr
	cmd.Env = append(cmd.Environ(), "SLINK_HOME="+t.TempDir())
	cmd.SysProcAttr = &syscall.SysProcAttr{Setpgid: true}
	if err := cmd.Start(); err != nil {
		t.Fatalf("start: %v", err)
	}
	for deadline := time.Now().Add(3 * time.Second); time.Now().Before(deadline); {
		if strings.Contains(stderr.String(), "session.link proxy") {
			break
		}
		time.Sleep(50 * time.Millisecond)
	}
	time.Sleep(200 * time.Millisecond) // let the child exec
	done := make(chan error, 1)
	go func() { done <- cmd.Wait() }()
	return cmd, &stderr, done
}

func waitOrKill(t *testing.T, cmd *exec.Cmd, done chan error, within time.Duration) {
	t.Helper()
	select {
	case <-done:
	case <-time.After(within):
		syscall.Kill(-cmd.Process.Pid, syscall.SIGKILL)
		t.Fatalf("slink dev did not exit within %v", within)
	}
}

// A terminal Ctrl-C signals the whole foreground process group. The wrapper
// must let the child die (it inherits default SIGINT handling), finalize,
// and exit 128+SIGINT — not ignore the signal (the v0.3.0 blocker).
func TestDevWrapperGroupSIGINT(t *testing.T) {
	bin := buildSlink(t)
	cmd, stderr, done := startWrapped(t, bin)
	if err := syscall.Kill(-cmd.Process.Pid, syscall.SIGINT); err != nil {
		t.Fatalf("killpg: %v", err)
	}
	waitOrKill(t, cmd, done, 5*time.Second)
	if code := cmd.ProcessState.ExitCode(); code != 130 {
		t.Errorf("exit code = %d, want 130 (128+SIGINT)\nstderr:\n%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "captured 0 LLM calls") {
		t.Errorf("missing zero-capture summary\nstderr:\n%s", stderr.String())
	}
}

// SIGTERM aimed at slink alone must be forwarded to the child so the whole
// run winds down and the capture still finalizes.
func TestDevWrapperForwardsSIGTERM(t *testing.T) {
	bin := buildSlink(t)
	cmd, stderr, done := startWrapped(t, bin)
	if err := cmd.Process.Signal(syscall.SIGTERM); err != nil {
		t.Fatalf("signal: %v", err)
	}
	waitOrKill(t, cmd, done, 5*time.Second)
	if code := cmd.ProcessState.ExitCode(); code != 143 {
		t.Errorf("exit code = %d, want 143 (128+SIGTERM)\nstderr:\n%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "captured 0 LLM calls") {
		t.Errorf("missing zero-capture summary\nstderr:\n%s", stderr.String())
	}
}

// An impatient second Ctrl-C must not kill the wrapper before the capture
// summary — the handler stays registered through Finalize.
func TestDevWrapperSurvivesDoubleSIGINT(t *testing.T) {
	bin := buildSlink(t)
	cmd, stderr, done := startWrapped(t, bin)
	if err := syscall.Kill(-cmd.Process.Pid, syscall.SIGINT); err != nil {
		t.Fatalf("killpg: %v", err)
	}
	time.Sleep(100 * time.Millisecond)
	syscall.Kill(cmd.Process.Pid, syscall.SIGINT) // aimed at slink alone
	waitOrKill(t, cmd, done, 5*time.Second)
	if code := cmd.ProcessState.ExitCode(); code != 130 {
		t.Errorf("exit code = %d, want 130\nstderr:\n%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "captured 0 LLM calls") {
		t.Errorf("second SIGINT killed the wrapper before the summary\nstderr:\n%s", stderr.String())
	}
}

// SIGINT aimed at the wrapper pid alone (not the group) must stop the run:
// it is forwarded to the child rather than swallowed.
func TestDevWrapperForwardsDirectSIGINT(t *testing.T) {
	bin := buildSlink(t)
	cmd, stderr, done := startWrapped(t, bin)
	if err := cmd.Process.Signal(syscall.SIGINT); err != nil {
		t.Fatalf("signal: %v", err)
	}
	waitOrKill(t, cmd, done, 5*time.Second)
	if code := cmd.ProcessState.ExitCode(); code != 130 {
		t.Errorf("exit code = %d, want 130\nstderr:\n%s", code, stderr.String())
	}
	if !strings.Contains(stderr.String(), "captured 0 LLM calls") {
		t.Errorf("missing summary after direct SIGINT\nstderr:\n%s", stderr.String())
	}
}
