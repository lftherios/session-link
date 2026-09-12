package cli

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/lftherios/session-link/internal/sealed"
)

func TestEncryptedUploadAndLostAcknowledgment(t *testing.T) {
	t.Setenv("SLINK_HOME", t.TempDir())
	plain := `{"schema":"session/v0","created_at":"2026-09-12T00:00:00Z","name":"PRIVATE_TITLE_SENTINEL","spans":[{"id":"root","type":"agent","raw":{"unknown":"retained"}}]}`
	var bodies [][]byte
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/api/shares" || r.URL.RawQuery != "" || r.Header.Get("Authorization") != "Bearer rk_test" || r.Header.Get("content-type") != "application/vnd.session-link.encrypted" {
			t.Error("unexpected upload request")
		}
		body, _ := io.ReadAll(r.Body)
		bodies = append(bodies, body)
		if bytes.Contains(body, []byte("PRIVATE_TITLE_SENTINEL")) || sealed.Validate(body) != nil {
			t.Error("upload is not ciphertext")
		}
		// The key must already be recoverable before a server receives any bytes.
		pending, _ := filepath.Glob(filepath.Join(Home(), "pending-shares", "*.json"))
		if len(pending) != 1 {
			t.Errorf("pending receipts: %d", len(pending))
		}
		if len(bodies) == 1 {
			w.WriteHeader(503)
			return
		}
		hash := sha256.Sum256(body)
		json.NewEncoder(w).Encode(map[string]any{"id": "23456789abcdef", "sha256": hex.EncodeToString(hash[:]), "url": "https://untrusted.example/s/23456789abcdef"})
	}))
	defer server.Close()
	if got := UploadRun(plain, server.URL, "rk_test"); got.OK {
		t.Fatal("unacknowledged upload reported success")
	}
	got := UploadRun(plain, server.URL, "rk_test")
	if !got.OK || len(bodies) != 2 || !bytes.Equal(bodies[0], bodies[1]) {
		t.Fatalf("retry did not recover exact ciphertext: %+v", got)
	}
	link, _ := url.Parse(got.Body["url"].(string))
	fragment, _ := url.ParseQuery(link.Fragment)
	key := fragment.Get("key")
	if !strings.HasPrefix(link.String(), server.URL+"/s/") {
		t.Fatal("key redirected to another origin")
	}
	decoded, err := sealed.Decrypt(bodies[1], key)
	if err != nil || string(decoded) != plain {
		t.Fatalf("decrypted bytes changed: %v", err)
	}
	receipts, _ := filepath.Glob(filepath.Join(Home(), "shares", "*.json"))
	if len(receipts) != 1 {
		t.Fatal("missing receipt")
	}
	stat, _ := os.Stat(receipts[0])
	if stat.Mode().Perm() != 0o600 {
		t.Fatal("key file is not private")
	}
	pending, _ := filepath.Glob(filepath.Join(Home(), "pending-shares", "*.json"))
	if len(pending) != 0 {
		t.Fatal("pending receipt not cleared")
	}
}

func TestEncryptedUploadRefusesUnsafeInputAndNoDowngrade(t *testing.T) {
	t.Setenv("SLINK_HOME", t.TempDir())
	requests := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests++
		if r.URL.Path != "/api/shares" {
			t.Error("plaintext fallback")
		}
		w.WriteHeader(404)
	}))
	defer server.Close()
	for _, text := range []string{`{}`, `{"schema":"session/v0","created_at":"2026-09-12T00:00:00Z","spans":[{"id":"r","type":"agent","raw":"sk-abcdefghijklmnop1234"}]}`} {
		if UploadRun(text, server.URL, "rk_test").OK {
			t.Fatal("unsafe input accepted")
		}
	}
	if requests != 0 {
		t.Fatal("unsafe input left the client")
	}
	valid := `{"schema":"session/v0","created_at":"2026-09-12T00:00:00Z","spans":[{"id":"root","type":"agent"}]}`
	if UploadRun(valid, server.URL, "rk_test").OK || requests != 1 {
		t.Fatal("old server must fail without downgrade")
	}
}
