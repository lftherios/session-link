package open

import (
	"net/http/httptest"
	"net/url"
	"strings"
	"testing"
)

func TestLocalCapabilityProtectsReadsAndWrites(t *testing.T) {
	s := &Server{Project: "/private/project", PreviewDir: t.TempDir()}
	for _, route := range []string{"/", "/settings", "/shared", "/p/saved", "/api/document/saved", "/api/login/status", "/api/stop", "/api/identity"} {
		for _, token := range []string{"", "wrong", (&Server{}).accessToken()} {
			r := httptest.NewRequest("GET", "http://127.0.0.1:4400"+route, nil)
			r.Header.Set("x-slink", "1")
			r.Header.Set("x-slink-access", token)
			w := httptest.NewRecorder()
			s.ServeHTTP(w, r)
			if w.Code != 403 || strings.Contains(w.Body.String(), s.Project) {
				t.Fatalf("unauthenticated %s: %d", route, w.Code)
			}
		}
	}
	r := httptest.NewRequest("GET", "http://127.0.0.1:4400/", nil)
	r.Header.Set("Sec-Fetch-Mode", "navigate")
	w := httptest.NewRecorder()
	s.ServeHTTP(w, r)
	if w.Code != 200 || strings.Contains(w.Body.String(), s.Project) || strings.Contains(w.Body.String(), s.accessToken()) {
		t.Fatal("navigation shell leaked private data")
	}
	if w.Header().Get("X-Frame-Options") != "DENY" || !strings.Contains(w.Header().Get("Content-Security-Policy"), "frame-ancestors 'none'") || w.Header().Get("Referrer-Policy") != "no-referrer" {
		t.Fatal("missing browser isolation")
	}
	r.Header.Set("x-slink-access", s.accessToken())
	w = httptest.NewRecorder()
	s.ServeHTTP(w, r)
	if w.Code != 200 || !strings.Contains(w.Body.String(), s.Project) {
		t.Fatal("authorized reader denied")
	}
	address, _ := url.Parse(s.AccessURL("http://127.0.0.1:4400/p/saved#span=step-1"))
	fragment, _ := url.ParseQuery(address.Fragment)
	if address.RawQuery != "" || fragment.Get("access") != s.accessToken() || fragment.Get("span") != "step-1" {
		t.Fatal("capability must stay in the fragment and preserve anchors")
	}
}
