package open

import (
	"context"
	"crypto/rand"
	"encoding/hex"
	"encoding/json"
	"net/http"
	"time"

	"github.com/lftherios/session-link/internal/cli"
)

type viewerLogin struct {
	ID     string `json:"id"`
	State  string `json:"state"`
	URL    string `json:"url,omitempty"`
	Code   string `json:"user_code,omitempty"`
	Error  string `json:"error,omitempty"`
	cancel context.CancelFunc
}

func (s *Server) loginAPI(w http.ResponseWriter, r *http.Request) {
	w.Header().Set("content-type", "application/json")
	if !localAction(r) {
		http.Error(w, `{"error":{"message":"cross-origin request blocked"}}`, 403)
		return
	}
	s.loginMu.Lock()
	defer s.loginMu.Unlock()
	switch {
	case r.URL.Path == "/api/login/status" && r.Method == "GET":
		if s.login != nil {
			json.NewEncoder(w).Encode(struct {
				*viewerLogin
				SignedIn bool   `json:"signed_in"`
				Server   string `json:"server"`
			}{s.login, s.apiKey() != "", s.Target})
		} else {
			json.NewEncoder(w).Encode(map[string]any{"state": "idle", "signed_in": s.apiKey() != "", "server": s.Target})
		}
	case r.URL.Path == "/api/login/cancel" && r.Method == "POST":
		if s.login != nil {
			s.login.cancel()
			s.login = nil
		}
		w.Write([]byte(`{"ok":true}`))
	case r.URL.Path == "/api/login/start" && r.Method == "POST":
		if s.login != nil && s.login.State == "pending" {
			json.NewEncoder(w).Encode(struct {
				*viewerLogin
				SignedIn bool   `json:"signed_in"`
				Server   string `json:"server"`
			}{s.login, s.apiKey() != "", s.Target})
			return
		}
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Minute)
		attempt, err := cli.BeginLogin(ctx, s.Target, "viewer")
		if err != nil {
			cancel()
			w.WriteHeader(502)
			json.NewEncoder(w).Encode(map[string]any{"error": map[string]string{"message": err.Error()}})
			return
		}
		token := make([]byte, 24)
		rand.Read(token)
		login := &viewerLogin{ID: hex.EncodeToString(token), State: "pending", URL: attempt.URL, Code: attempt.UserCode, cancel: cancel}
		s.login = login
		go func() {
			defer cancel()
			_, err := cli.WaitLogin(ctx, attempt)
			s.loginMu.Lock()
			defer s.loginMu.Unlock()
			if s.login != login {
				return
			}
			login.URL, login.Code = "", ""
			if err != nil {
				login.State, login.Error = "error", err.Error()
			} else {
				login.State = "complete"
			}
		}()
		json.NewEncoder(w).Encode(login)
	default:
		http.NotFound(w, r)
	}
}
