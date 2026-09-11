package open

import (
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/lftherios/session-link/internal/handoff"
	"github.com/lftherios/session-link/internal/share"
)

type composeSource struct {
	run     map[string]any
	catalog share.Catalog
}
type savedDraft struct {
	Revision int64       `json:"revision"`
	Draft    share.Draft `json:"draft"`
}
type savedViewRecord struct {
	Source  string       `json:"source"`
	Draft   *share.Draft `json:"draft,omitempty"`
	SavedAt int64        `json:"saved_at,omitempty"`
}
type savedViewLink struct {
	Title   string `json:"title"`
	URL     string `json:"url"`
	SavedAt int64  `json:"-"`
}

func (s *Server) viewRecord(id string) (savedViewRecord, error) {
	var record savedViewRecord
	if !fileID.MatchString(id) || id == "." || id == ".." {
		return record, fmt.Errorf("invalid saved view")
	}
	raw, err := os.ReadFile(filepath.Join(s.draftsDir(), "exports", id+".json"))
	if err != nil {
		return record, err
	}
	err = json.Unmarshal(raw, &record)
	return record, err
}

func (s *Server) savedViews(source string) []savedViewLink {
	views := []savedViewLink{}
	entries, _ := os.ReadDir(filepath.Join(s.draftsDir(), "exports"))
	for _, entry := range entries {
		if entry.IsDir() || !strings.HasSuffix(entry.Name(), ".json") {
			continue
		}
		id := strings.TrimSuffix(entry.Name(), ".json")
		record, err := s.viewRecord(id)
		if err != nil || record.Source != source || record.Draft == nil {
			continue
		}
		if _, err := os.Stat(filepath.Join(s.previewDir(), id+".json")); err != nil {
			continue
		}
		views = append(views, savedViewLink{Title: record.Draft.Title, URL: "/p/" + id, SavedAt: record.SavedAt})
	}
	sort.Slice(views, func(i, j int) bool { return views[i].SavedAt > views[j].SavedAt })
	return views
}

func (s *Server) draftsDir() string { return filepath.Join(filepath.Dir(s.previewDir()), "drafts") }
func (s *Server) localTitle(id string) string {
	raw, err := os.ReadFile(filepath.Join(s.draftsDir(), "titles", id+".json"))
	if err != nil {
		return ""
	}
	var title struct {
		Title string `json:"title"`
	}
	if json.Unmarshal(raw, &title) != nil {
		return ""
	}
	return title.Title
}

// sessionTitleKey names the title file shared by every snapshot of a session.
func sessionTitleKey(harness, id string) string {
	if harness == "" || id == "" {
		return ""
	}
	sum := sha256.Sum256([]byte(harness + "\n" + id))
	return "session-" + hex.EncodeToString(sum[:16])
}

func runSessionTitleKey(run map[string]any) string {
	source, _ := run["source"].(map[string]any)
	meta, _ := run["metadata"].(map[string]any)
	harness, _ := source["harness"].(string)
	id, _ := meta["session_id"].(string)
	return sessionTitleKey(harness, id)
}

// titleFor prefers the title typed for the session over the snapshot's own,
// so a later snapshot of the same session keeps its name.
func (s *Server) titleFor(id string, run map[string]any) string {
	if key := runSessionTitleKey(run); key != "" {
		if title := s.localTitle(key); title != "" {
			return title
		}
	}
	return s.localTitle(id)
}

// typedSessionTitle is the title typed on any snapshot of a session.
func (s *Server) typedSessionTitle(harness, id string) string {
	if key := sessionTitleKey(harness, id); key != "" {
		return s.localTitle(key)
	}
	return ""
}
func (s *Server) composeSource(id string) (*composeSource, error) {
	if cached, ok := s.composeSources.Load(id); ok {
		return cached.(*composeSource), nil
	}
	raw, err := os.ReadFile(filepath.Join(s.previewDir(), id+".json"))
	if err != nil {
		return nil, err
	}
	var run map[string]any
	if err := json.Unmarshal(raw, &run); err != nil {
		return nil, err
	}
	source := &composeSource{run: run, catalog: share.BuildCatalog(run)}
	actual, _ := s.composeSources.LoadOrStore(id, source)
	return actual.(*composeSource), nil
}

func (s *Server) readDraft(id string) (savedDraft, error) {
	d := savedDraft{Draft: share.Draft{Title: "Shared session excerpt", Items: []share.Selection{}}}
	dir := filepath.Join(s.draftsDir(), id)
	files, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return d, nil
	}
	if err != nil {
		return d, err
	}
	var newest int64
	var file string
	for _, entry := range files {
		if !entry.IsDir() && strings.HasSuffix(entry.Name(), ".json") {
			version, err := strconv.ParseInt(strings.TrimSuffix(entry.Name(), ".json"), 10, 64)
			if err == nil && version > newest {
				newest, file = version, entry.Name()
			}
		}
	}
	if file == "" {
		return d, nil
	}
	raw, err := os.ReadFile(filepath.Join(dir, file))
	if err != nil {
		return d, err
	}
	err = json.Unmarshal(raw, &d)
	return d, err
}

// A complete revision is installed only if that revision number is unused.
// Hard-link creation is atomic across viewer processes, unlike an in-process
// mutex or replacing one shared draft file. Readers never see partial JSON.
func (s *Server) writeDraft(id string, draft savedDraft) error {
	dir := filepath.Join(s.draftsDir(), id)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	data, err := json.Marshal(draft)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(dir, ".draft-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err := f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Link(f.Name(), filepath.Join(dir, fmt.Sprintf("%020d.json", draft.Revision)))
}

func atomicJSON(file string, value any) error {
	if err := os.MkdirAll(filepath.Dir(file), 0o700); err != nil {
		return err
	}
	data, err := json.Marshal(value)
	if err != nil {
		return err
	}
	f, err := os.CreateTemp(filepath.Dir(file), ".draft-*")
	if err != nil {
		return err
	}
	defer os.Remove(f.Name())
	if _, err := f.Write(data); err != nil {
		f.Close()
		return err
	}
	if err := f.Close(); err != nil {
		return err
	}
	return os.Rename(f.Name(), file)
}

func decodeDraftRequest(w http.ResponseWriter, r *http.Request, target any) error {
	r.Body = http.MaxBytesReader(w, r.Body, 2*1024*1024)
	d := json.NewDecoder(r.Body)
	d.DisallowUnknownFields()
	if err := d.Decode(target); err != nil {
		return fmt.Errorf("cannot read draft: %w", err)
	}
	var extra any
	if err := d.Decode(&extra); err != io.EOF {
		return fmt.Errorf("expected one draft document")
	}
	return nil
}

func (s *Server) composeAPI(w http.ResponseWriter, r *http.Request, action, id string) {
	send := func(status int, value any) {
		w.Header().Set("content-type", "application/json")
		w.WriteHeader(status)
		json.NewEncoder(w).Encode(value)
	}
	fail := func(status int, err error) {
		send(status, map[string]any{"error": map[string]any{"message": err.Error()}})
	}
	read := (action == "api/compose" || action == "api/title") && r.Method == http.MethodGet
	if !read && !(((action == "api/draft" || action == "api/title") && r.Method == http.MethodPut) || (action == "api/export" && r.Method == http.MethodPost)) {
		fail(405, fmt.Errorf("method not allowed"))
		return
	}
	if !read && !localAction(r) {
		fail(403, fmt.Errorf("cross-origin request blocked"))
		return
	}
	source, err := s.composeSource(id)
	if err != nil {
		fail(404, fmt.Errorf("the saved source preview is unavailable; open the session again with slink view"))
		return
	}
	if action == "api/title" {
		if read {
			send(200, map[string]string{"title": s.titleFor(id, source.run)})
			return
		}
		var title struct {
			Title string `json:"title"`
		}
		if err := decodeDraftRequest(w, r, &title); err != nil {
			fail(400, err)
			return
		}
		title.Title = strings.TrimSpace(title.Title)
		if title.Title == "" || utf8.RuneCountInString(title.Title) > 256 || strings.ContainsAny(title.Title, "\n\r") {
			fail(422, fmt.Errorf("use a title of 1–256 characters on one line"))
			return
		}
		if err := atomicJSON(filepath.Join(s.draftsDir(), "titles", id+".json"), title); err != nil {
			fail(500, err)
			return
		}
		// The session's own title file lets later snapshots and the sessions page use it.
		if key := runSessionTitleKey(source.run); key != "" {
			if err := atomicJSON(filepath.Join(s.draftsDir(), "titles", key+".json"), title); err != nil {
				fail(500, err)
				return
			}
		}
		send(200, title)
		return
	}
	if read {
		draft, err := s.readDraft(id)
		if err != nil {
			fail(500, err)
			return
		}
		if view := r.URL.Query().Get("view"); view != "" {
			record, err := s.viewRecord(view)
			if err != nil || record.Source != id || record.Draft == nil {
				fail(404, fmt.Errorf("the saved view is unavailable for this session"))
				return
			}
			draft.Draft = *record.Draft
		}
		title := s.titleFor(id, source.run)
		if title == "" {
			title, _ = source.run["name"].(string)
		}
		send(200, map[string]any{"catalog": source.catalog, "source_title": title, "revision": draft.Revision, "draft": draft.Draft, "views": s.savedViews(id)})
		return
	}
	if action == "api/draft" {
		var next savedDraft
		if err := decodeDraftRequest(w, r, &next); err != nil {
			fail(400, err)
			return
		}
		if err := next.Draft.Validate(source.catalog, false); err != nil {
			fail(422, err)
			return
		}
		s.draftMu.Lock()
		defer s.draftMu.Unlock()
		current, err := s.readDraft(id)
		if err != nil {
			fail(500, err)
			return
		}
		if current.Revision != next.Revision {
			fail(409, fmt.Errorf("this draft changed in another tab; your edits are still here, but reload before saving further changes"))
			return
		}
		next.Revision++
		if err := s.writeDraft(id, next); err != nil {
			if errors.Is(err, os.ErrExist) {
				fail(409, fmt.Errorf("this draft changed in another viewer; reload before saving further changes"))
				return
			}
			fail(500, err)
			return
		}
		send(200, map[string]any{"revision": next.Revision})
		return
	}
	var draft share.Draft
	if err := decodeDraftRequest(w, r, &draft); err != nil {
		fail(400, err)
		return
	}
	doc, err := share.Export(source.run, source.catalog, draft)
	if err != nil {
		fail(422, err)
		return
	}
	raw, err := json.Marshal(doc)
	if err != nil {
		fail(500, err)
		return
	}
	exported, err := handoff.Save(s.previewDir(), handoff.Source{Read: func() ([]byte, error) { return raw, nil }})
	if err != nil {
		fail(500, err)
		return
	}
	// Editing provenance stays local, entirely outside the share document.
	if err := atomicJSON(filepath.Join(s.draftsDir(), "exports", exported+".json"), savedViewRecord{Source: id, Draft: &draft, SavedAt: time.Now().UnixNano()}); err != nil {
		fail(500, err)
		return
	}
	send(200, map[string]any{"url": "/p/" + exported, "document": doc})
}

func (s *Server) composePage(id string) (string, error) {
	if _, err := s.composeSource(id); err != nil {
		return "", err
	}
	config, _ := json.Marshal(map[string]string{"source": id})
	return page("Prepare an excerpt · session.link", `<div id="root"></div><script>window.__COMPOSE__=`+string(config)+`</script><script src="/assets/viewer.js"></script>`), nil
}

func (s *Server) editSource(id string) string {
	raw, err := os.ReadFile(filepath.Join(s.draftsDir(), "exports", id+".json"))
	if err != nil {
		return ""
	}
	var record savedViewRecord
	if json.Unmarshal(raw, &record) != nil || !fileID.MatchString(record.Source) {
		return ""
	}
	return record.Source
}
