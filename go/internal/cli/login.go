package cli

import (
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"time"
)

// WriteConfig persists ~/.slink/config.json at 0600 — it holds the API key.
func WriteConfig(c Config) (string, error) {
	dir := Home()
	if err := os.MkdirAll(dir, 0o755); err != nil {
		return "", err
	}
	file := filepath.Join(dir, "config.json")
	b, err := json.MarshalIndent(c, "", "  ")
	if err != nil {
		return "", err
	}
	if err := os.WriteFile(file, append(b, '\n'), 0o600); err != nil {
		return "", err
	}
	os.Chmod(file, 0o600) // mode only applies on create
	return file, nil
}

// LoginResult reports how the login concluded.
type LoginResult struct {
	ConfigPath string
	Login      string
	PrevServer string
}

// LoginWithKey is the paste path — CI, or operator-minted keys.
func LoginWithKey(key, server string) (*LoginResult, error) {
	if len(key) < 3 || key[:3] != "rk_" {
		return nil, fmt.Errorf("that doesn't look like an API key (rk_…)")
	}
	c := ReadConfig()
	c.APIKey = key
	if server != "" {
		c.Server = server
	}
	file, err := WriteConfig(c)
	if err != nil {
		return nil, err
	}
	return &LoginResult{ConfigPath: file}, nil
}

// BrowserLogin mirrors the JS flow: mint a code, open the approve page,
// poll until the key drops out. notify receives progress lines for stderr.
func BrowserLogin(server string, notify func(string)) (*LoginResult, error) {
	client := &http.Client{Timeout: 30 * time.Second}
	res, err := client.Post(server+"/api/auth/cli", "application/json", nil)
	if err != nil {
		return nil, fmt.Errorf("cannot reach %s: %v", server, err)
	}
	var out struct {
		Code     string `json:"code"`
		UserCode string `json:"user_code"`
		URL      string `json:"url"`
		Error    *struct {
			Message string `json:"message"`
		} `json:"error"`
	}
	json.NewDecoder(res.Body).Decode(&out)
	res.Body.Close()
	if res.StatusCode == 503 {
		msg := "browser login isn't configured on this server — pass --key rk_…"
		if out.Error != nil && out.Error.Message != "" {
			msg = out.Error.Message
		}
		return nil, fmt.Errorf("%s", msg)
	}
	if res.StatusCode < 200 || res.StatusCode >= 300 || out.Code == "" {
		if out.Error != nil {
			return nil, fmt.Errorf("login failed — %s", out.Error.Message)
		}
		return nil, fmt.Errorf("login failed — HTTP %d", res.StatusCode)
	}

	notify(fmt.Sprintf("Opening %s", out.URL))
	if out.UserCode != "" {
		notify(fmt.Sprintf("  confirm this code in the browser: %s", out.UserCode))
	}
	notify("  if the browser doesn't open, visit the URL yourself")
	OpenBrowser(out.URL)

	deadline := time.Now().Add(10 * time.Minute)
	misses := 0
	for time.Now().Before(deadline) {
		time.Sleep(2 * time.Second)
		poll, err := client.Get(server + "/api/auth/cli/" + out.Code)
		if err != nil {
			// Keep polling through blips, but don't pretend all is well forever.
			if misses++; misses == 5 {
				notify(fmt.Sprintf("  still trying to reach %s (%v)…", server, err))
			}
			continue
		}
		misses = 0
		if poll.StatusCode == http.StatusAccepted {
			poll.Body.Close()
			continue
		}
		if poll.StatusCode == http.StatusNotFound {
			poll.Body.Close()
			return nil, fmt.Errorf("login code expired — run `slink login` again")
		}
		if poll.StatusCode < 200 || poll.StatusCode >= 300 {
			poll.Body.Close()
			return nil, fmt.Errorf("login failed — HTTP %d", poll.StatusCode)
		}
		var grant struct {
			Key   string `json:"key"`
			Login string `json:"login"`
		}
		json.NewDecoder(poll.Body).Decode(&grant)
		poll.Body.Close()
		if grant.Key == "" {
			return nil, fmt.Errorf("login failed — unexpected response from the server; run `slink login` again")
		}
		c := ReadConfig()
		prev := c.Server
		c.APIKey = grant.Key
		c.Server = server
		file, err := WriteConfig(c)
		if err != nil {
			return nil, err
		}
		return &LoginResult{ConfigPath: file, Login: grant.Login, PrevServer: prev}, nil
	}
	if misses >= 5 {
		return nil, fmt.Errorf("timed out — never reached %s; check the URL and run `slink login` again", server)
	}
	return nil, fmt.Errorf("timed out waiting for browser approval — run `slink login` again")
}

// OpenBrowser is best-effort — the URL is always printed too.
func OpenBrowser(url string) {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", url)
	case "windows":
		cmd = exec.Command("rundll32", "url.dll,FileProtocolHandler", url)
	default:
		cmd = exec.Command("xdg-open", url)
	}
	cmd.Start()
}
