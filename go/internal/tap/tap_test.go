package tap

import (
	"bytes"
	"context"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

/* ----------------------------------------------------------- router unit */

func testSession(t *testing.T, name string) *Session {
	t.Helper()
	return NewSession(t.TempDir(), name, time.Now())
}

func TestRouterIdleAndBusySemantics(t *testing.T) {
	now := time.Unix(0, 0)
	clock := func() time.Time { return now }
	var finalized []*Session
	r := NewRouter(100*time.Millisecond, clock,
		func(name string) *Session { return testSession(t, name) },
		func(s *Session) { finalized = append(finalized, s) },
	)

	s1 := r.Route("k", "a")
	if got := r.Route("k", "b"); got != s1 {
		t.Fatal("within the idle window the session is reused")
	}
	// Busy sessions survive any staleness.
	s1.Begin()
	now = now.Add(time.Hour)
	if got := r.Route("k", "c"); got != s1 {
		t.Fatal("busy session must be reused, not rolled over")
	}
	r.Sweep()
	if len(finalized) != 0 {
		t.Fatal("busy session must not be swept")
	}
	s1.End()
	now = now.Add(time.Hour) // the busy reuse bumped last-seen; go idle again
	r.Sweep()
	// The sweep finalizes asynchronously; wait for it.
	deadline := time.Now().Add(2 * time.Second)
	for len(finalized) == 0 && time.Now().Before(deadline) {
		time.Sleep(10 * time.Millisecond)
	}
	if len(finalized) != 1 {
		t.Fatalf("idle quiet session must be swept, got %d", len(finalized))
	}
	if r.Size() != 0 {
		t.Fatal("swept session must leave the map")
	}
}

/* ------------------------------------------------------------ daemon e2e */

func TestTapEndToEnd(t *testing.T) {
	upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		body, _ := json.Marshal(map[string]any{
			"id": "m1", "object": "chat.completion", "model": "mock-1",
			"usage": map[string]any{"prompt_tokens": 7, "completion_tokens": 3},
			"choices": []any{map[string]any{
				"index": 0, "finish_reason": "stop",
				"message": map[string]any{"role": "assistant", "content": "ok"},
			}},
		})
		w.Header().Set("content-type", "application/json")
		w.Write(body)
	}))
	defer upstream.Close()
	t.Setenv("SLINK_UPSTREAM_OPENAI", upstream.URL)

	dir := t.TempDir()
	srv := NewServer(dir, time.Minute)
	ts := httptest.NewServer(srv)
	defer ts.Close()

	req := `{"model":"mock-1","messages":[{"role":"user","content":"tap e2e"}]}`
	for i := 0; i < 2; i++ {
		resp, err := http.Post(ts.URL+"/openai/v1/chat/completions", "application/json", strings.NewReader(req))
		if err != nil {
			t.Fatal(err)
		}
		if resp.StatusCode != 200 {
			t.Fatalf("proxied status %d", resp.StatusCode)
		}
		var out map[string]any
		json.NewDecoder(resp.Body).Decode(&out)
		resp.Body.Close()
		if out["object"] != "chat.completion" {
			t.Fatal("response not passed through")
		}
	}

	// During recording: spool + sidecar, no json yet.
	spools, _ := filepath.Glob(filepath.Join(dir, "*.json.spool"))
	if len(spools) != 1 {
		t.Fatalf("expected 1 spool during recording, got %d", len(spools))
	}

	srv.router.FinalizeAll()

	captures, _ := filepath.Glob(filepath.Join(dir, "*.json"))
	if len(captures) != 1 {
		t.Fatalf("expected 1 finalized capture, got %d", len(captures))
	}
	raw, err := os.ReadFile(captures[0])
	if err != nil {
		t.Fatal(err)
	}
	var doc struct {
		Schema   string `json:"schema"`
		Name     string `json:"name"`
		Metadata struct {
			InProgress *bool `json:"in_progress"`
		} `json:"metadata"`
		Spans []struct {
			ID     string         `json:"id"`
			Type   string         `json:"type"`
			Status string         `json:"status"`
			Usage  map[string]any `json:"usage"`
		} `json:"spans"`
	}
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatalf("capture is not valid JSON: %v", err)
	}
	if doc.Schema != "session/v0" || doc.Metadata.InProgress != nil {
		t.Fatalf("bad envelope: schema=%q in_progress=%v", doc.Schema, doc.Metadata.InProgress)
	}
	if doc.Name != "tap e2e" {
		t.Fatalf("derived name: %q", doc.Name)
	}
	if len(doc.Spans) != 3 || doc.Spans[0].Type != "agent" || doc.Spans[1].ID != "s1" || doc.Spans[2].ID != "s2" {
		t.Fatalf("span tree wrong: %+v", doc.Spans)
	}
	if doc.Spans[1].Status != "ok" || doc.Spans[1].Usage["input_tokens"] != float64(7) {
		t.Fatalf("span content wrong: %+v", doc.Spans[1])
	}
	if bytes.Contains(raw, []byte(`"in_progress"`)) {
		t.Fatal("in_progress must be gone from the finalized capture")
	}

	// Shutdown path exercises Serve()'s ordering separately.
	ctx, cancel := context.WithCancel(context.Background())
	cancel()
	if err := NewServer(t.TempDir(), time.Minute).Serve(ctx, "127.0.0.1:0"); err != nil {
		t.Fatalf("clean shutdown: %v", err)
	}
}
