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
