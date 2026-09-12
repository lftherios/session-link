package cli

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"runtime"
	"strings"
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
	if err := WritePrivate(file, append(b, '\n')); err != nil {
		return "", err
	}
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
	c.Login, c.UserID = "", ""
	if server != "" {
		c.Server = server
	}
	file, err := WriteConfig(c)
	if err != nil {
		return nil, err
	}
	return &LoginResult{ConfigPath: file}, nil
}

// LoginAttempt contains a short-lived grant; it must never be logged in full.
type LoginAttempt struct {
	Code     string `json:"-"`
	UserCode string `json:"user_code"`
	URL      string `json:"url"`
	Server   string `json:"-"`
}

func loginClient() *http.Client {
	return &http.Client{Timeout: 30 * time.Second, CheckRedirect: func(_ *http.Request, _ []*http.Request) error { return http.ErrUseLastResponse }}
}

func BeginLogin(ctx context.Context, server, source string) (*LoginAttempt, error) {
	u, err := url.Parse(server)
	if err != nil || u.Host == "" || u.User != nil || u.RawQuery != "" || u.Fragment != "" ||
		(u.Scheme != "https" && !(u.Scheme == "http" && (u.Hostname() == "localhost" || net.ParseIP(u.Hostname()).IsLoopback()))) {
		return nil, fmt.Errorf("sign-in requires an HTTPS server (or localhost)")
	}
	server = strings.TrimRight(server, "/")
	body, _ := json.Marshal(map[string]string{"source": source})
	req, err := http.NewRequestWithContext(ctx, "POST", server+"/api/auth/cli", bytes.NewReader(body))
	if err != nil {
		return nil, err
	}
	req.Header.Set("content-type", "application/json")
	res, err := loginClient().Do(req)
	if err != nil {
		return nil, fmt.Errorf("cannot reach the sign-in server")
	}
	defer res.Body.Close()
	var out struct {
		Code     string `json:"code"`
		UserCode string `json:"user_code"`
	}
	if json.NewDecoder(io.LimitReader(res.Body, 8192)).Decode(&out) != nil || res.StatusCode != 200 || len(out.Code) != 43 || out.UserCode == "" {
		return nil, fmt.Errorf("cannot start sign-in (HTTP %d); check that email or GitHub login is configured", res.StatusCode)
	}
	if raw, err := base64.RawURLEncoding.DecodeString(out.Code); err != nil || len(raw) != 32 || base64.RawURLEncoding.EncodeToString(raw) != out.Code {
		return nil, fmt.Errorf("invalid sign-in response")
	}
	// Use the configured server; never follow an arbitrary response URL.
	return &LoginAttempt{Code: out.Code, UserCode: out.UserCode, URL: server + "/cli/" + out.Code, Server: server}, nil
}

func WaitLogin(ctx context.Context, attempt *LoginAttempt) (*LoginResult, error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Minute)
	defer cancel()
	ticker := time.NewTicker(2 * time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return nil, fmt.Errorf("sign-in expired or was cancelled; try again")
		case <-ticker.C:
		}
		req, err := http.NewRequestWithContext(ctx, "GET", attempt.Server+"/api/auth/cli/"+attempt.Code, nil)
		if err != nil {
			return nil, err
		}
		res, err := loginClient().Do(req)
		if err != nil {
			continue
		}
		var grant struct {
			Key   string `json:"key"`
			Login string `json:"login"`
			UID   string `json:"uid"`
		}
		decodeErr := json.NewDecoder(io.LimitReader(res.Body, 8192)).Decode(&grant)
		res.Body.Close()
		if res.StatusCode == http.StatusAccepted {
			continue
		}
		if res.StatusCode == 404 {
			return nil, fmt.Errorf("sign-in expired; try again")
		}
		if decodeErr != nil || res.StatusCode != 200 || !strings.HasPrefix(grant.Key, "rk_") {
			return nil, fmt.Errorf("sign-in failed (HTTP %d)", res.StatusCode)
		}
		if ctx.Err() != nil {
			return nil, ctx.Err()
		}
		c := ReadConfig()
		prev := c.Server
		c.APIKey, c.Server, c.Login, c.UserID = grant.Key, attempt.Server, grant.Login, grant.UID
		file, err := WriteConfig(c)
		if err != nil {
			return nil, err
		}
		return &LoginResult{ConfigPath: file, Login: grant.Login, PrevServer: prev}, nil
	}
}

// BrowserLogin remains the terminal entry point; the viewer uses Begin/Wait.
func BrowserLogin(server string, notify func(string)) (*LoginResult, error) {
	attempt, err := BeginLogin(context.Background(), server, "terminal")
	if err != nil {
		return nil, err
	}
	notify(fmt.Sprintf("Opening %s\n  confirm this code in the browser: %s", attempt.URL, attempt.UserCode))
	notify("  if the browser doesn't open, visit the URL yourself")
	OpenBrowser(attempt.URL)
	return WaitLogin(context.Background(), attempt)
}

// OpenBrowser is best-effort — the URL is always printed too.
func OpenBrowser(url string) error {
	var cmd *exec.Cmd
	switch runtime.GOOS {
	case "darwin":
		cmd = exec.Command("open", url)
	case "windows":
		cmd = exec.Command("rundll32", "url.dll,FileProtocolHandler", url)
	default:
		cmd = exec.Command("xdg-open", url)
	}
	if err := cmd.Start(); err != nil {
		return err
	}
	go cmd.Wait()
	return nil
}
