package open

import (
	"bytes"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"

	"github.com/lftherios/session-link/internal/cli"
	"github.com/lftherios/session-link/internal/handoff"
)

func action(s *Server, method, path, origin, header string) *httptest.ResponseRecorder {
	r := httptest.NewRequest(method, "http://127.0.0.1:4400"+path, nil)
	r.Header.Set("Origin", origin)
	r.Header.Set("x-slink", header)
	w := httptest.NewRecorder()
	s.ServeHTTP(w, r)
	return w
}

func previewFixture(t *testing.T) []byte {
	t.Helper()
	data, err := os.ReadFile("../../../testdata/import/pi/basic/golden.run.json")
	if err != nil {
		t.Fatal(err)
	}
	return data
}

func TestPreviewSelectionAndPublishUseSavedBytes(t *testing.T) {
	data := previewFixture(t)
	reads, uploads := 0, 0
	var received []byte
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		uploads++
		if r.URL.Path != "/api/runs" || r.Header.Get("Authorization") != "Bearer fixture-key" {
			t.Errorf("unexpected upload %v", r)
		}
		received, _ = io.ReadAll(r.Body)
		if uploads == 1 {
			w.WriteHeader(503)
			io.WriteString(w, `{"error":{"message":"Retry later"}}`)
			return
		}
		io.WriteString(w, `{"url":"https://example.test/r/saved"}`)
	}))
	defer upstream.Close()
	var published string
	s := &Server{Project: "/work/project", PreviewDir: t.TempDir(), Target: upstream.URL, APIKey: "fixture-key",
		Sources:   []handoff.Source{{ID: "pinned", Harness: "pi", Title: "Research <script>", Read: func() ([]byte, error) { reads++; return data, nil }}},
		OnPublish: func(url string) { published = url },
	}
	index := action(s, "GET", "/", "", "")
	if index.Code != 200 || !strings.Contains(index.Body.String(), "Research &lt;script&gt;") {
		t.Fatal(index.Body.String())
	}
	if reads != 0 || uploads != 0 {
		t.Fatal("opening the picker read or uploaded the session")
	}
	rebound := httptest.NewRecorder()
	s.ServeHTTP(rebound, httptest.NewRequest("GET", "http://untrusted.test:4400/", nil))
	if rebound.Code != 403 {
		t.Fatal("untrusted hostname can read local sessions")
	}
	for _, request := range []struct {
		method, origin, header string
		status                 int
	}{
		{"GET", "", "1", 404}, {"POST", "", "", 403},
		{"POST", "http://127.0.0.1:4401", "1", 403}, {"POST", "https://example.test", "1", 403},
	} {
		w := action(s, request.method, "/api/preview/0", request.origin, request.header)
		if w.Code != request.status {
			t.Fatalf("%+v: %d", request, w.Code)
		}
	}
	if reads != 0 {
		t.Fatal("rejected request read the session")
	}
	w := action(s, "POST", "/api/preview/0", "http://127.0.0.1:4400", "1")
	var body struct {
		URL string `json:"url"`
	}
	if err := json.Unmarshal(w.Body.Bytes(), &body); err != nil || w.Code != 200 {
		t.Fatalf("%s %v", w.Body, err)
	}
	if reads != 1 || uploads != 0 {
		t.Fatal("preview must read exactly the selected source without upload")
	}
	file := filepath.Join(s.PreviewDir, strings.TrimPrefix(body.URL, "/p/")+".json")
	saved, err := os.ReadFile(file)
	if err != nil {
		t.Fatal(err)
	}
	data = []byte(`{"new":"agent work that must not be published"}`)
	page := action(s, "GET", body.URL, "", "")
	if page.Code != 200 || !strings.Contains(page.Body.String(), "notes.txt") || strings.Contains(page.Body.String(), "agent work") {
		t.Fatal("preview did not use the saved document")
	}
	endpoint := strings.Replace(body.URL, "/p/", "/api/publish-preview/", 1)
	for _, want := range []int{503, 200} {
		w = action(s, "POST", endpoint, "http://127.0.0.1:4400", "1")
		if w.Code != want {
			t.Fatalf("publish: %d %s", w.Code, w.Body)
		}
		if !bytes.Equal(saved, received) {
			t.Fatal("uploaded bytes differ from preview")
		}
	}
	if reads != 1 || published != "https://example.test/r/saved" {
		t.Fatalf("read=%d published=%q", reads, published)
	}
	for _, origin := range []string{"http://127.0.0.1:4401", "https://example.test"} {
		if w := action(s, "POST", endpoint, origin, "1"); w.Code != 403 {
			t.Fatal(w.Code)
		}
	}
	if uploads != 2 {
		t.Fatal("rejected publish reached the network")
	}
}

func TestPreviewSecretGateAndMissingSource(t *testing.T) {
	data := bytes.Replace(previewFixture(t), []byte("notes.txt has 12 lines."), []byte("sk-"+strings.Repeat("a", 48)), 1)
	s := &Server{Project: "/work/project", PreviewDir: t.TempDir(), Target: "http://127.0.0.1:1", APIKey: "fixture-key",
		Sources: []handoff.Source{{Read: func() ([]byte, error) { return data, nil }}, {Read: func() ([]byte, error) { return nil, os.ErrNotExist }}},
	}
	w := action(s, "POST", "/api/preview/0", "", "1")
	var body struct {
		URL string `json:"url"`
	}
	json.Unmarshal(w.Body.Bytes(), &body)
	if w.Code != 200 {
		t.Fatal(w.Body.String())
	}
	w = action(s, "POST", strings.Replace(body.URL, "/p/", "/api/publish-preview/", 1), "", "1")
	if w.Code != 422 || !strings.Contains(w.Body.String(), "secrets_detected") {
		t.Fatalf("%d %s", w.Code, w.Body)
	}
	if w = action(s, "POST", "/api/preview/1", "", "1"); w.Code != 422 {
		t.Fatal(w.Code)
	}
	if w = action(s, "POST", "/api/preview/2", "", "1"); w.Code != 404 {
		t.Fatal(w.Code)
	}
	stopped := false
	s.OnStop = func() { stopped = true }
	action(s, "POST", "/api/stop", "http://127.0.0.1:4401", "1")
	if stopped {
		t.Fatal("cross-origin request stopped the viewer")
	}
	action(s, "POST", "/api/stop", "", "1")
	if !stopped {
		t.Fatal("local stop did not stop viewer")
	}
}

func TestSignInReloadKeepsPreparedPreview(t *testing.T) {
	t.Setenv("SLINK_HOME", t.TempDir())
	t.Setenv("SLINK_API_KEY", "")
	t.Setenv("SLINK_SERVER", "http://127.0.0.1:9")
	data := previewFixture(t)
	s := &Server{PreviewDir: t.TempDir(), Target: "http://127.0.0.1:9"}
	id, err := handoff.Save(s.PreviewDir, handoff.Source{Read: func() ([]byte, error) { return data, nil }})
	if err != nil {
		t.Fatal(err)
	}
	before := action(s, "GET", "/p/"+id, "", "")
	if !strings.Contains(before.Body.String(), `"hasKey":false`) {
		t.Fatal("unexpected credentials")
	}
	config := cli.ReadConfig()
	config.Server, config.APIKey = s.Target, "new-key"
	if _, err := cli.WriteConfig(config); err != nil {
		t.Fatal(err)
	}
	s.APIKey = "expired-key" // A previously supplied key must not mask a new login.
	after := action(s, "GET", "/p/"+id, "", "")
	if after.Code != 200 || !strings.Contains(after.Body.String(), `"hasKey":true`) || s.apiKey() != "new-key" {
		t.Fatal("reload did not pick up login")
	}
	if strings.Contains(after.Body.String(), "new-key") {
		t.Fatal("API key leaked into page")
	}
	if !strings.Contains(after.Body.String(), "notes.txt has 12 lines.") {
		t.Fatal("login replaced preview")
	}
}
