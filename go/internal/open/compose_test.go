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
	"github.com/lftherios/session-link/internal/share"
)

func composeRequest(s *Server, method, path string, value any) *httptest.ResponseRecorder {
	data, _ := json.Marshal(value)
	r := httptest.NewRequest(method, "http://127.0.0.1:4400"+path, bytes.NewReader(data))
	r.Header.Set("Origin", "http://127.0.0.1:4400")
	r.Header.Set("x-slink", "1")
	w := httptest.NewRecorder()
	s.ServeHTTP(w, r)
	return w
}

func researchSource(t *testing.T, s *Server) (string, share.Draft) {
	t.Helper()
	raw, err := os.ReadFile("../../../testdata/share/research/session.json")
	if err != nil {
		t.Fatal(err)
	}
	id, err := handoff.Save(s.PreviewDir, handoff.Source{Read: func() ([]byte, error) { return raw, nil }})
	if err != nil {
		t.Fatal(err)
	}
	raw, err = os.ReadFile("../../../testdata/share/research/draft.json")
	if err != nil {
		t.Fatal(err)
	}
	var draft share.Draft
	if err := json.Unmarshal(raw, &draft); err != nil {
		t.Fatal(err)
	}
	return id, draft
}

func TestLocalTitlePersistsWithoutRewritingSourceOrDraft(t *testing.T) {
	s := &Server{PreviewDir: t.TempDir()}
	id, draft := researchSource(t, s)
	file := filepath.Join(s.PreviewDir, id+".json")
	before, _ := os.ReadFile(file)
	w := composeRequest(s, "PUT", "/api/title/"+id, map[string]string{"title": "Review onboarding differences"})
	if w.Code != 200 {
		t.Fatal(w.Code, w.Body.String())
	}
	s = &Server{PreviewDir: s.PreviewDir}
	if s.localTitle(id) != "Review onboarding differences" {
		t.Fatal("title lost after restart")
	}
	w = action(s, "GET", "/api/compose/"+id, "", "")
	if !strings.Contains(w.Body.String(), `"source_title":"Review onboarding differences"`) {
		t.Fatal(w.Body.String())
	}
	if w := composeRequest(s, "PUT", "/api/draft/"+id, savedDraft{Draft: draft}); w.Code != 200 {
		t.Fatal(w.Body.String())
	}
	composeRequest(s, "PUT", "/api/title/"+id, map[string]string{"title": "A new local title"})
	saved, _ := s.readDraft(id)
	if saved.Draft.Title != draft.Title {
		t.Fatal("local rename overwrote the prepared share title")
	}
	after, _ := os.ReadFile(file)
	if !bytes.Equal(before, after) {
		t.Fatal("local rename rewrote captured bytes")
	}
	page := action(s, "GET", "/p/"+id, "", "")
	if !strings.Contains(page.Body.String(), `window.__LOCAL__=`) || !strings.Contains(page.Body.String(), "A new local title") {
		t.Fatal("saved local title missing from viewer configuration")
	}
	for _, title := range []string{"", strings.Repeat("a", 257), "line\nbreak"} {
		if w := composeRequest(s, "PUT", "/api/title/"+id, map[string]string{"title": title}); w.Code != 422 {
			t.Fatal("invalid title accepted", w.Code)
		}
	}
	if w := action(s, "PUT", "/api/title/"+id, "", ""); w.Code != 403 {
		t.Fatal("unguarded title write", w.Code)
	}
}

func TestComposeDraftPersistenceAndExactExport(t *testing.T) {
	s := &Server{PreviewDir: t.TempDir(), Target: "http://127.0.0.1:1"}
	source, draft := researchSource(t, s)
	if w := action(s, "GET", "/compose/"+source, "", ""); w.Code != 200 || !strings.Contains(w.Body.String(), "__COMPOSE__") {
		t.Fatal(w.Body.String())
	}
	if w := action(s, "GET", "/api/compose/"+source, "", ""); w.Code != 200 || !strings.Contains(w.Body.String(), "Compare Atlas") {
		t.Fatal(w.Body.String())
	}
	save := composeRequest(s, "PUT", "/api/draft/"+source, savedDraft{Revision: 0, Draft: draft})
	if save.Code != 200 {
		t.Fatal(save.Body.String())
	}
	// A new server process restores the sender's title, note and selections.
	s = &Server{PreviewDir: s.PreviewDir, Target: s.Target}
	w := action(s, "GET", "/api/compose/"+source, "", "")
	var restored savedDraft
	if err := json.Unmarshal(w.Body.Bytes(), &restored); err != nil {
		t.Fatal(err)
	}
	if restored.Revision != 1 || restored.Draft.Note != draft.Note || len(restored.Draft.Items) != 2 {
		t.Fatalf("draft not restored: %+v", restored)
	}
	// The stale tab is rejected rather than silently replacing saved edits.
	w = composeRequest(s, "PUT", "/api/draft/"+source, savedDraft{Revision: 0, Draft: share.Draft{Title: "stale"}})
	if w.Code != 409 {
		t.Fatalf("stale write: %d %s", w.Code, w.Body)
	}
	w = composeRequest(s, "POST", "/api/export/"+source, draft)
	if w.Code != 200 {
		t.Fatal(w.Body.String())
	}
	var result struct {
		URL string `json:"url"`
	}
	json.Unmarshal(w.Body.Bytes(), &result)
	id := strings.TrimPrefix(result.URL, "/p/")
	file := filepath.Join(s.PreviewDir, id+".json")
	exported, err := os.ReadFile(file)
	if err != nil {
		t.Fatal(err)
	}
	if bytes.Contains(exported, []byte("OMITTED_INTERNAL_STRATEGY")) || bytes.Contains(exported, []byte(source)) {
		t.Fatal("source content or local provenance leaked")
	}
	page := action(s, "GET", result.URL, "", "")
	if page.Code != 200 || strings.Contains(page.Body.String(), "OMITTED_INTERNAL_STRATEGY") || !strings.Contains(page.Body.String(), "Edit selection") {
		t.Fatal("export page mixed in source material or lost edit path")
	}
	if s.editSource(id) != source {
		t.Fatal("local edit provenance lost")
	}
	// The public upload route stays closed for the new envelope until hosted
	// compatibility is verified. Whole-session publishing keeps its own tests.
	w = action(s, "POST", "/api/publish-preview/"+id, "", "1")
	if w.Code != 409 {
		t.Fatalf("excerpt publishing was enabled: %d %s", w.Code, w.Body)
	}
	// Validate the prepared bytes against the existing upload contract using
	// only a mock receiver; excluded data must not reach that receiver either.
	var received []byte
	receiver := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		received, _ = io.ReadAll(r.Body)
		io.WriteString(w, `{"url":"https://example.test/r/excerpt"}`)
	}))
	defer receiver.Close()
	ins, err := cli.InspectRunFile(file)
	if err != nil || len(ins.Errors) != 0 || len(ins.Secrets) != 0 {
		t.Fatalf("inspection: %+v %v", ins, err)
	}
	if uploaded := cli.UploadRun(ins.Text, receiver.URL, "fixture-key"); !uploaded.OK {
		t.Fatal(uploaded)
	}
	if !bytes.Equal(received, exported) {
		t.Fatal("upload bytes differ from the prepared excerpt")
	}
	// Editing and exporting again creates another preview, leaving this one intact.
	draft.Note = "A revised request for review"
	w = composeRequest(s, "POST", "/api/export/"+source, draft)
	var changed struct {
		URL string `json:"url"`
	}
	json.Unmarshal(w.Body.Bytes(), &changed)
	if w.Code != 200 || changed.URL == result.URL {
		t.Fatal("updated note did not create an independent document")
	}
	again, _ := os.ReadFile(file)
	if !bytes.Equal(again, exported) {
		t.Fatal("editing replaced the original export")
	}
}

func TestComposeRejectsInvalidAndCrossOriginMutations(t *testing.T) {
	s := &Server{PreviewDir: t.TempDir()}
	id, draft := researchSource(t, s)
	for _, path := range []string{"/api/draft/", "/api/export/"} {
		method := "POST"
		if strings.Contains(path, "draft") {
			method = "PUT"
		}
		if w := action(s, method, path+id, "https://untrusted.test", "1"); w.Code != 403 {
			t.Fatal(w.Code)
		}
		if w := action(s, method, path+id, "", ""); w.Code != 403 {
			t.Fatal(w.Code)
		}
		if w := action(s, "GET", path+id, "", ""); w.Code != 405 {
			t.Fatal(w.Code)
		}
	}
	draft.Items[0].ID = "not-a-source-item"
	if w := composeRequest(s, "POST", "/api/export/"+id, draft); w.Code != 422 {
		t.Fatal(w.Body.String())
	}
	if w := composeRequest(s, "PUT", "/api/draft/"+id, savedDraft{Draft: draft}); w.Code != 422 {
		t.Fatal(w.Body.String())
	}
	if w := composeRequest(s, "POST", "/api/export/missing", draft); w.Code != 404 {
		t.Fatal(w.Code)
	}
	if w := composeRequest(s, "POST", "/api/export/"+id, map[string]any{"title": "a", "unexpected": "field"}); w.Code != 400 {
		t.Fatal(w.Code)
	}
}

func TestDraftConcurrentViewersCannotOverwriteEachOther(t *testing.T) {
	s := &Server{PreviewDir: t.TempDir()}
	id, draft := researchSource(t, s)
	other := &Server{PreviewDir: s.PreviewDir}
	start := make(chan struct{})
	results := make(chan int, 2)
	for _, server := range []*Server{s, other} {
		go func() {
			<-start
			results <- composeRequest(server, "PUT", "/api/draft/"+id, savedDraft{Draft: draft}).Code
		}()
	}
	close(start)
	a, b := <-results, <-results
	if !((a == 200 && b == 409) || (a == 409 && b == 200)) {
		t.Fatalf("concurrent writes: %d %d", a, b)
	}
	saved, err := s.readDraft(id)
	if err != nil || saved.Revision != 1 || saved.Draft.Note != draft.Note {
		t.Fatalf("invalid committed revision: %+v %v", saved, err)
	}
}

func TestSavedViewsRemainIndependentAndReopenAfterRestart(t *testing.T) {
	s := &Server{PreviewDir: t.TempDir()}
	source, draft := researchSource(t, s)
	original, _ := os.ReadFile(filepath.Join(s.PreviewDir, source+".json"))
	working := savedDraft{Draft: draft}
	working.Draft.Title = "Unfinished working draft"
	if w := composeRequest(s, "PUT", "/api/draft/"+source, working); w.Code != 200 {
		t.Fatal(w.Body.String())
	}
	save := func(d share.Draft) (string, []byte) {
		t.Helper()
		w := composeRequest(s, "POST", "/api/export/"+source, d)
		if w.Code != 200 {
			t.Fatal(w.Body.String())
		}
		var result struct {
			URL      string          `json:"url"`
			Document json.RawMessage `json:"document"`
		}
		if err := json.Unmarshal(w.Body.Bytes(), &result); err != nil {
			t.Fatal(err)
		}
		id := strings.TrimPrefix(result.URL, "/p/")
		raw, err := os.ReadFile(filepath.Join(s.PreviewDir, id+".json"))
		if err != nil || !bytes.Equal(mustCompact(raw), mustCompact(result.Document)) {
			t.Fatal("download does not match saved view", err)
		}
		if bytes.Contains(raw, []byte(source)) || bytes.Contains(raw, []byte("OMITTED_INTERNAL_STRATEGY")) {
			t.Fatal("saved view leaked source material")
		}
		return id, raw
	}
	first, firstBytes := save(draft)
	draft.Title, draft.Note = "Second view", "A different question"
	second, _ := save(draft)
	if first == second {
		t.Fatal("a changed view replaced the previous snapshot")
	}
	s = &Server{PreviewDir: s.PreviewDir}
	views := s.savedViews(source)
	if len(views) != 2 || views[0].Title != "Second view" {
		t.Fatalf("saved view index lost after restart: %+v", views)
	}
	if len(s.savedViews("another-source")) != 0 {
		t.Fatal("saved views crossed session boundaries")
	}
	after, _ := os.ReadFile(filepath.Join(s.PreviewDir, first+".json"))
	if !bytes.Equal(after, firstBytes) {
		t.Fatal("earlier saved view changed")
	}
	after, _ = os.ReadFile(filepath.Join(s.PreviewDir, source+".json"))
	if !bytes.Equal(after, original) {
		t.Fatal("source capture changed")
	}
	persisted, _ := s.readDraft(source)
	if persisted.Revision != 1 || persisted.Draft.Title != "Unfinished working draft" {
		t.Fatal("saving a view overwrote the working draft")
	}
	w := action(s, "GET", "/api/compose/"+source+"?view="+second, "", "")
	var loaded savedDraft
	if w.Code != 200 || json.Unmarshal(w.Body.Bytes(), &loaded) != nil || loaded.Draft.Title != "Second view" || loaded.Draft.Note != "A different question" {
		t.Fatal("editing did not restore the requested saved view", w.Body.String())
	}
	w = action(s, "GET", "/api/compose/"+source+"?view=unknown", "", "")
	if w.Code != 404 {
		t.Fatal("unknown view was silently substituted")
	}
}
