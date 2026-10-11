package open

import (
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"sync"
	"testing"

	"github.com/lftherios/session-link/internal/identity"
)

func TestLocalIdentityAndLoginRequireActionHeaderAndOrigin(t *testing.T) {
	server := &Server{Target: "http://127.0.0.1:1", APIKey: "must-stay-native"}
	for _, route := range []string{"/api/login/status", "/api/login/start", "/api/login/cancel", "/api/identity"} {
		for _, origin := range []string{"", "https://attacker.test", "null"} {
			req := httptest.NewRequest("POST", "http://127.0.0.1:4321"+route, strings.NewReader(`{"action":"status"}`))
			if origin != "" {
				req.Header.Set("x-slink", "1")
				req.Header.Set("origin", origin)
			}
			response := httptest.NewRecorder()
			req.Header.Set("X-Slink-Access", server.accessToken())
			server.ServeHTTP(response, req)
			if response.Code != 403 {
				t.Fatalf("%s origin=%s: status %d", route, origin, response.Code)
			}
			if strings.Contains(response.Body.String(), server.APIKey) {
				t.Fatal("API key leaked")
			}
		}
	}
	req := httptest.NewRequest("GET", "http://127.0.0.1:4321/api/login/status", nil)
	req.Header.Set("x-slink", "1")
	res := httptest.NewRecorder()
	req.Header.Set("X-Slink-Access", server.accessToken())
	server.ServeHTTP(res, req)
	if res.Code != 200 || strings.Contains(res.Body.String(), server.APIKey) {
		t.Fatal("safe sign-in status failed")
	}
}

// The settings page offers to discard an unfinished setup when it sees this
// code, so the code has to survive the trip through the local API.
func TestLocalIdentityReportsUnfinishedSetup(t *testing.T) {
	var mu sync.Mutex
	down, record := true, json.RawMessage("null")
	remote := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		mu.Lock()
		defer mu.Unlock()
		if r.Method == "POST" {
			if down {
				http.Error(w, `{"error":{"message":"unavailable"}}`, 503)
				return
			}
			var body struct{ Event, Vault json.RawMessage }
			if json.NewDecoder(r.Body).Decode(&body) != nil {
				http.Error(w, `{"error":{"message":"invalid"}}`, 400)
				return
			}
			record, _ = json.Marshal(map[string]any{"events": []json.RawMessage{body.Event}, "vault": body.Vault})
		}
		json.NewEncoder(w).Encode(map[string]any{"account": "usr_local", "record": record, "requests": []any{}})
	}))
	defer remote.Close()
	t.Setenv("SLINK_HOME", t.TempDir())
	server := &Server{Target: remote.URL, APIKey: "fixture"}
	call := func(action string) (int, string, string) {
		t.Helper()
		req := httptest.NewRequest("POST", "http://127.0.0.1:4321/api/identity", strings.NewReader(`{"action":"`+action+`"}`))
		req.Header.Set("x-slink", "1")
		req.Header.Set("X-Slink-Access", server.accessToken())
		res := httptest.NewRecorder()
		server.ServeHTTP(res, req)
		var body struct {
			State string `json:"state"`
			Error struct {
				Code string `json:"code"`
			} `json:"error"`
		}
		if err := json.Unmarshal(res.Body.Bytes(), &body); err != nil {
			t.Fatal(err)
		}
		return res.Code, body.State, body.Error.Code
	}
	if status, _, code := call("setup"); status == 200 || code != "" {
		t.Fatalf("expected an interrupted setup: %d %q", status, code)
	}
	mu.Lock()
	down = false
	mu.Unlock()
	other := identity.Client{Home: t.TempDir(), Server: remote.URL, APIKey: "fixture"}
	if _, err := other.Handle(context.Background(), "setup", identity.Input{}); err != nil {
		t.Fatal(err)
	}
	if status, _, code := call("status"); status != 422 || code != "setup_conflict" {
		t.Fatalf("unfinished setup was not reported: %d %q", status, code)
	}
	if status, state, _ := call("discard-setup"); status != 200 || state != "locked" {
		t.Fatalf("unfinished setup was not discarded: %d %q", status, state)
	}
	if status, state, _ := call("status"); status != 200 || state != "locked" {
		t.Fatalf("device did not become a new device: %d %q", status, state)
	}
}
