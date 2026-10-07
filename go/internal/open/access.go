package open

import (
	"crypto/rand"
	"crypto/subtle"
	_ "embed"
	"encoding/base64"
	"net/http"
	"net/url"
	"strings"
)

//go:embed local-access.js
var localAccessJS string

func (s *Server) accessToken() string {
	s.accessOnce.Do(func() {
		key := make([]byte, 32)
		if _, err := rand.Read(key); err != nil {
			panic(err) // Never start an unprotected viewer if randomness fails.
		}
		s.accessKey = base64.RawURLEncoding.EncodeToString(key)
	})
	return s.accessKey
}

// AccessURL puts the local capability in a fragment, never an HTTP URL or cookie.
func (s *Server) AccessURL(address string) string {
	u, err := url.Parse(address)
	if err != nil {
		panic(err)
	}
	fragment, _ := url.ParseQuery(u.Fragment)
	fragment.Set("access", s.accessToken())
	u.Fragment = fragment.Encode()
	return u.String()
}

func (s *Server) authorized(r *http.Request) bool {
	return subtle.ConstantTimeCompare([]byte(r.Header.Get("X-Slink-Access")), []byte(s.accessToken())) == 1
}

func securityHeaders(w http.ResponseWriter) {
	w.Header().Set("Cache-Control", "no-store")
	w.Header().Set("Referrer-Policy", "no-referrer")
	w.Header().Set("X-Content-Type-Options", "nosniff")
	w.Header().Set("X-Frame-Options", "DENY")
	w.Header().Set("Content-Security-Policy", "default-src 'none'; script-src 'self' 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; connect-src 'self'; base-uri 'none'; object-src 'none'; form-action 'none'; frame-ancestors 'none'")
}

// Ordinary navigations get a public shell. It fetches the private HTML using
// the fragment key; callers without it never receive session or account data.
func accessPage(r *http.Request) bool {
	if r.Method != http.MethodGet || r.Header.Get("Sec-Fetch-Mode") != "navigate" {
		return false
	}
	p := r.URL.Path
	return p == "/" || p == "/settings" || p == "/shared" || strings.HasPrefix(p, "/p/") || strings.HasPrefix(p, "/r/") || strings.HasPrefix(p, "/compose/")
}

const accessShell = `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1"><title>Local session viewer</title><p id="access-status">Opening local viewer…</p><script src="/assets/local-access.js" data-bootstrap></script>`
