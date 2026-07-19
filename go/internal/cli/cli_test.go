package cli

import (
	"compress/gzip"
	"encoding/json"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/lftherios/session-link/internal/format"
	"github.com/lftherios/session-link/internal/scan"
)

// Every golden the JS reference validates clean must validate clean here —
// the oracle doubles as the schema-parity test.
func TestValidateRunOverGoldens(t *testing.T) {
	wd, _ := os.Getwd()
	root := filepath.Join(wd, "..", "..", "..")
	matches, _ := filepath.Glob(filepath.Join(root, "testdata", "import", "*", "*", "golden.run.json"))
	if len(matches) == 0 {
		t.Fatal("no goldens found")
	}
	for _, f := range matches {
		raw, err := os.ReadFile(f)
		if err != nil {
			t.Fatal(err)
		}
		var data any
		if err := json.Unmarshal(raw, &data); err != nil {
			t.Fatal(err)
		}
		if errs := format.ValidateRun(data); len(errs) != 0 {
			t.Fatalf("%s: %v", f, errs[0])
		}
	}
	// Format keywords must be ASSERTED (ajv-formats parity): a malformed
	// timestamp is rejected client-side, not first by the server.
	badDate := map[string]any{
		"schema": "session/v0", "created_at": "not-a-date-time",
		"spans": []any{map[string]any{"id": "root", "type": "agent"}},
	}
	if errs := format.ValidateRun(any(badDate)); len(errs) == 0 {
		t.Fatal("malformed created_at must be rejected")
	}
	// And an invalid document must produce errors, not a printer panic.
	badType := map[string]any{
		"schema": "session/v0", "created_at": "2026-07-19T10:00:00Z",
		"spans": []any{map[string]any{"id": "root", "type": "agent", "parent_id": 42}},
	}
	if errs := format.ValidateRun(any(badType)); len(errs) == 0 {
		t.Fatal("numeric parent_id must be rejected")
	}

	// And the invariants the schema can't express.
	bad := map[string]any{
		"schema": "session/v0", "created_at": "2026-07-19T10:00:00Z",
		"spans": []any{
			map[string]any{"id": "a", "parent_id": "b", "type": "custom"},
			map[string]any{"id": "b", "parent_id": "a", "type": "custom"},
		},
	}
	if errs := format.ValidateRun(any(bad)); len(errs) == 0 || !strings.Contains(errs[0], "cycle") {
		t.Fatalf("cycle must be rejected, got %v", errs)
	}
	dup := map[string]any{
		"schema": "session/v0", "created_at": "2026-07-19T10:00:00Z",
		"spans": []any{
			map[string]any{"id": "a", "type": "custom"},
			map[string]any{"id": "a", "type": "custom"},
		},
	}
	if errs := format.ValidateRun(any(dup)); len(errs) == 0 || !strings.Contains(errs[0], "duplicate") {
		t.Fatalf("duplicate ids must be rejected, got %v", errs)
	}
}

func TestScanParityWithJS(t *testing.T) {
	hits := scan.ForSecrets("here is sk-abcdefghijklmnop1234 and sk_live_abcdefghijklmnop1234 ok")
	if len(hits) != 2 {
		t.Fatalf("expected 2 hits, got %+v", hits)
	}
	// The exact preview format the JS scanner produces.
	if hits[0].Pattern != "provider_api_key" || hits[0].Preview != "sk-abcde…(23 chars)" {
		t.Fatalf("preview parity: %+v", hits[0])
	}
	if hits[1].Pattern != "stripe_key" || hits[1].Preview != "sk_live_…(28 chars)" {
		t.Fatalf("preview parity: %+v", hits[1])
	}
	if got := scan.ForSecrets(strings.Repeat("sk-abcdefghijklmnop1234 ", 50)); len(got) != 10 {
		t.Fatalf("hit cap: %d", len(got))
	}
}

func TestUploadRunGzipsAndAuths(t *testing.T) {
	var gotAuth, gotEncoding string
	var gotBody []byte
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		gotAuth = r.Header.Get("authorization")
		gotEncoding = r.Header.Get("content-encoding")
		body, _ := io.ReadAll(r.Body)
		if gotEncoding == "gzip" {
			zr, _ := gzip.NewReader(strings.NewReader(string(body)))
			body, _ = io.ReadAll(zr)
		}
		gotBody = body
		w.WriteHeader(201)
		json.NewEncoder(w).Encode(map[string]any{"id": "x", "url": "https://s/r/x"})
	}))
	defer ts.Close()

	small := `{"schema":"session/v0"}`
	res := UploadRun(small, ts.URL, "rk_test")
	if !res.OK || gotAuth != "Bearer rk_test" || gotEncoding != "" || string(gotBody) != small {
		t.Fatalf("small upload: %+v auth=%q enc=%q", res, gotAuth, gotEncoding)
	}
	big := `{"pad":"` + strings.Repeat("x", 70*1024) + `"}`
	res = UploadRun(big, ts.URL, "")
	if !res.OK || gotEncoding != "gzip" || string(gotBody) != big {
		t.Fatalf("big upload must gzip and round-trip: enc=%q ok=%v", gotEncoding, res.OK)
	}
}

func TestPlanPruneParity(t *testing.T) {
	now := time.Date(2026, 7, 19, 12, 0, 0, 0, time.UTC)
	caps := []Capture{
		{File: "d", CreatedAt: "2026-07-19T11:00:00Z", Spans: 5, InProgress: true},
		{File: "c", CreatedAt: "2026-07-19T10:00:00Z", Spans: 1},
		{File: "b", CreatedAt: "2026-06-01T10:00:00Z", Spans: 9},
		{File: "a", CreatedAt: "2026-01-01T10:00:00Z", Spans: 3},
	}
	remove, kept := PlanPrune(caps, now, 30*24*time.Hour, -1, false)
	if len(remove) != 2 || remove[0].File != "b" || remove[1].File != "a" {
		t.Fatalf("age window: %+v", remove)
	}
	if len(kept) != 2 || !kept[0].InProgress {
		t.Fatalf("in-progress must be kept: %+v", kept)
	}
	remove, _ = PlanPrune(caps, now, -1, -1, true)
	if len(remove) != 1 || remove[0].File != "c" {
		t.Fatalf("empty-only: %+v", remove)
	}
	remove, _ = PlanPrune(caps, now, -1, 2, false)
	if len(remove) != 2 {
		t.Fatalf("keep 2: %+v", remove)
	}
}

func TestListSeesFinalizedAndSpooled(t *testing.T) {
	dir := t.TempDir()
	// A finalized capture.
	fin := filepath.Join(dir, "20260719-100000-aaaaaa.json")
	os.WriteFile(fin, []byte(`{"schema":"session/v0","name":"done","created_at":"2026-07-19T10:00:00.000Z","metadata":{},"spans":[{"id":"root","type":"agent"},{"id":"s1","type":"llm_call","model":{"id":"m1"},"input":{"messages":[]},"output":{"messages":[]}}]}`), 0o644)
	captures := ListCaptures(dir)
	if len(captures) != 1 || captures[0].Name != "done" || captures[0].Spans != 2 || captures[0].Models[0] != "m1" {
		t.Fatalf("listing: %+v", captures)
	}
}

func TestServiceGenerators(t *testing.T) {
	plist := LaunchdPlist([]string{"/usr/local/bin/slink", "tap", "--port", "4141"}, "/home/u/.slink/tap.log", ServiceLabel)
	for _, want := range []string{
		"<string>link.session.tap</string>",
		"<string>/usr/local/bin/slink</string>",
		"<string>--port</string>",
		"<key>KeepAlive</key>",
		"<string>/home/u/.slink/tap.log</string>",
	} {
		if !strings.Contains(plist, want) {
			t.Fatalf("plist missing %q", want)
		}
	}
	// XML escaping — a path with & must not corrupt the plist.
	esc := LaunchdPlist([]string{"/apps/a&b/slink"}, "/l.log", ServiceLabel)
	if !strings.Contains(esc, "a&amp;b") {
		t.Fatal("xml escaping missing")
	}
	unit := SystemdUnit([]string{"/opt/my apps/slink", "tap", "--port", "4141"})
	if !strings.Contains(unit, `ExecStart="/opt/my apps/slink" tap --port 4141`) {
		t.Fatalf("unit quoting: %s", unit)
	}
	if !strings.Contains(unit, "Restart=always") {
		t.Fatal("unit must restart")
	}
}

func TestBrowserLoginPollFlow(t *testing.T) {
	t.Setenv("SLINK_HOME", t.TempDir())
	polls := 0
	ts := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		switch {
		case r.Method == "POST" && r.URL.Path == "/api/auth/cli":
			json.NewEncoder(w).Encode(map[string]any{"code": "c1", "user_code": "AB-12", "url": "https://x/cli/c1"})
		case r.URL.Path == "/api/auth/cli/c1":
			polls++
			if polls < 2 {
				w.WriteHeader(202)
				return
			}
			json.NewEncoder(w).Encode(map[string]any{"key": "rk_granted", "login": "tester"})
		default:
			w.WriteHeader(404)
		}
	}))
	defer ts.Close()

	var notes []string
	r, err := BrowserLogin(ts.URL, func(s string) { notes = append(notes, s) })
	if err != nil {
		t.Fatal(err)
	}
	if r.Login != "tester" {
		t.Fatalf("login: %+v", r)
	}
	c := ReadConfig()
	if c.APIKey != "rk_granted" || c.Server != ts.URL {
		t.Fatalf("config not saved: %+v", c)
	}
	st, _ := os.Stat(r.ConfigPath)
	if st.Mode().Perm() != 0o600 {
		t.Fatalf("config perms: %v", st.Mode().Perm())
	}
	if len(notes) == 0 || !strings.Contains(notes[1], "AB-12") {
		t.Fatalf("user code must be surfaced: %v", notes)
	}
}
