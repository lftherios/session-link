package cli

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"runtime"
	"strings"
)

// Login-service install for the always-on tap — launchd on macOS,
// systemd --user on Linux. The plist/unit generators are pure and
// unit-tested; install/uninstall shell out to launchctl/systemctl.

const ServiceLabel = "link.session.tap"

var xmlEscaper = strings.NewReplacer(
	"&", "&amp;", "<", "&lt;", ">", "&gt;", "'", "&apos;", `"`, "&quot;",
)

// TapProgramArgs: the Go binary is self-contained — absolute path so the
// service never depends on the login PATH.
func TapProgramArgs(port int) []string {
	exe, err := os.Executable()
	if err != nil {
		exe = "slink"
	}
	return []string{exe, "tap", "--port", fmt.Sprintf("%d", port)}
}

func LaunchdPlist(programArgs []string, logPath, label string) string {
	lines := make([]string, len(programArgs))
	for i, a := range programArgs {
		lines[i] = "      <string>" + xmlEscaper.Replace(a) + "</string>"
	}
	return fmt.Sprintf(`<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
  <dict>
    <key>Label</key>
    <string>%s</string>
    <key>ProgramArguments</key>
    <array>
%s
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
    <key>StandardOutPath</key>
    <string>%s</string>
    <key>StandardErrorPath</key>
    <string>%s</string>
  </dict>
</plist>
`, label, strings.Join(lines, "\n"), xmlEscaper.Replace(logPath), xmlEscaper.Replace(logPath))
}

var hasSpace = regexp.MustCompile(`\s`)

func SystemdUnit(programArgs []string) string {
	quoted := make([]string, len(programArgs))
	for i, a := range programArgs {
		if hasSpace.MatchString(a) {
			quoted[i] = `"` + a + `"`
		} else {
			quoted[i] = a
		}
	}
	return fmt.Sprintf(`[Unit]
Description=session.link always-on tap
After=network.target

[Service]
ExecStart=%s
Restart=always

[Install]
WantedBy=default.target
`, strings.Join(quoted, " "))
}

// ServiceResult mirrors the JS install/uninstall return.
type ServiceResult struct {
	OK    bool
	Path  string
	Error string
}

func launchdPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, "Library", "LaunchAgents", ServiceLabel+".plist")
}

func systemdPath() string {
	home, _ := os.UserHomeDir()
	return filepath.Join(home, ".config", "systemd", "user", "slink-tap.service")
}

func TapLogPath() string { return filepath.Join(Home(), "tap.log") }

func InstallService(port int) ServiceResult {
	args := TapProgramArgs(port)
	switch runtime.GOOS {
	case "darwin":
		p := launchdPath()
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			return ServiceResult{Error: err.Error()}
		}
		os.MkdirAll(filepath.Dir(TapLogPath()), 0o755)
		if err := os.WriteFile(p, []byte(LaunchdPlist(args, TapLogPath(), ServiceLabel)), 0o644); err != nil {
			return ServiceResult{Error: err.Error()}
		}
		exec.Command("launchctl", "unload", p).Run() // reload-safe
		if err := exec.Command("launchctl", "load", p).Run(); err != nil {
			return ServiceResult{Error: "launchctl load failed: " + err.Error(), Path: p}
		}
		return ServiceResult{OK: true, Path: p}
	case "linux":
		p := systemdPath()
		if err := os.MkdirAll(filepath.Dir(p), 0o755); err != nil {
			return ServiceResult{Error: err.Error()}
		}
		if err := os.WriteFile(p, []byte(SystemdUnit(args)), 0o644); err != nil {
			return ServiceResult{Error: err.Error()}
		}
		exec.Command("systemctl", "--user", "daemon-reload").Run()
		if err := exec.Command("systemctl", "--user", "enable", "--now", "slink-tap.service").Run(); err != nil {
			return ServiceResult{Error: "systemctl enable failed: " + err.Error(), Path: p}
		}
		return ServiceResult{OK: true, Path: p}
	}
	return ServiceResult{Error: "login services are supported on macOS and Linux only"}
}

func UninstallService() ServiceResult {
	switch runtime.GOOS {
	case "darwin":
		p := launchdPath()
		exec.Command("launchctl", "unload", p).Run()
		os.Remove(p)
		return ServiceResult{OK: true, Path: p}
	case "linux":
		p := systemdPath()
		exec.Command("systemctl", "--user", "disable", "--now", "slink-tap.service").Run()
		os.Remove(p)
		exec.Command("systemctl", "--user", "daemon-reload").Run()
		return ServiceResult{OK: true, Path: p}
	}
	return ServiceResult{Error: "login services are supported on macOS and Linux only"}
}
