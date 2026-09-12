package open

import (
	"net/http/httptest"
	"strings"
	"testing"
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
	server.ServeHTTP(res, req)
	if res.Code != 200 || strings.Contains(res.Body.String(), server.APIKey) {
		t.Fatal("safe sign-in status failed")
	}
}
