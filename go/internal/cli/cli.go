// Package cli holds the Go CLI's command cores: config, capture listing,
// the publish pipeline, and retention — ports of cli/store.mjs (listing),
// cli/publish.mjs, and cli/prune.mjs.
package cli

import (
	"bytes"
	"compress/gzip"
	"encoding/json"
	"fmt"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strings"
	"time"

	"github.com/lftherios/session-link/internal/format"
	"github.com/lftherios/session-link/internal/scan"
	"github.com/lftherios/session-link/internal/spool"
)

/* --------------------------------------------------------------- config */

// Config is ~/.slink/config.json, written by `slink login`.
type Config struct {
	APIKey string `json:"api_key"`
	Server string `json:"server"`
	Login  string `json:"login,omitempty"` // GitHub handle, when browser login minted the key
}

func Home() string {
	if h := os.Getenv("SLINK_HOME"); h != "" {
		return h
	}
	home, err := os.UserHomeDir()
	if err != nil {
		return ".slink"
	}
	return filepath.Join(home, ".slink")
}

func CaptureDir() string { return filepath.Join(Home(), "runs") }

func ReadConfig() Config {
	var c Config
	b, err := os.ReadFile(filepath.Join(Home(), "config.json"))
	if err == nil {
		json.Unmarshal(b, &c)
	}
	return c
}

// ResolveTarget mirrors resolveTarget's precedence: flag, env, config, default.
func ResolveTarget(serverFlag, keyFlag string) (server, apiKey string) {
	c := ReadConfig()
	server = firstNonEmpty(serverFlag, os.Getenv("SLINK_SERVER"), c.Server, "https://session.link")
	apiKey = firstNonEmpty(keyFlag, os.Getenv("SLINK_API_KEY"), c.APIKey)
	return
}

func firstNonEmpty(vals ...string) string {
	for _, v := range vals {
		if v != "" {
			return v
		}
	}
	return ""
}

/* -------------------------------------------------------------- listing */

// Capture is one row of `slink list` — the JS listCaptures entry.
type Capture struct {
	File       string
	Name       string
	CreatedAt  string
	Spans      int
	Models     []string
	InProgress bool
}

// ListCaptures ports the JS listing pass: recover dead spools (which also
// runs the litter sweeps), snapshot live ones, then list .json captures,
// healing any stranded in_progress on the way.
func ListCaptures(dir string) []Capture {
	spool.RecoverDead(dir)

	entries, err := os.ReadDir(dir)
	if err != nil {
		return nil
	}
	// Live spools: materialize a snapshot when the spool is newer than the json.
	for _, e := range entries {
		if !strings.HasSuffix(e.Name(), ".json.spool") {
			continue
		}
		spoolFile := filepath.Join(dir, e.Name())
		capture := strings.TrimSuffix(spoolFile, ".spool")
		if !spool.OwnerAlive(capture) {
			continue // RecoverDead already finalized or set aside
		}
		sp, err := os.Stat(spoolFile)
		if err != nil {
			continue
		}
		js, err := os.Stat(capture)
		if err != nil || sp.ModTime().After(js.ModTime()) {
			spool.Assemble(capture, spool.AssembleOptions{}) // snapshot; best-effort
		}
	}

	entries, _ = os.ReadDir(dir) // snapshots may have added .json files
	out := []Capture{}
	for _, e := range entries {
		name := e.Name()
		if !strings.HasSuffix(name, ".json") {
			continue
		}
		file := filepath.Join(dir, name)
		raw, err := os.ReadFile(file)
		if err != nil {
			continue
		}
		var run map[string]any
		if json.Unmarshal(raw, &run) != nil {
			continue
		}
		schema, _ := run["schema"].(string)
		if schema != "session/v0" && schema != "run/v0" {
			continue
		}
		meta, _ := run["metadata"].(map[string]any)
		inProgress := meta != nil && meta["in_progress"] == true
		if inProgress {
			if _, err := os.Stat(spool.SpoolPath(file)); err != nil && !spool.OwnerAlive(file) {
				// Stranded — no spool, no live owner. RecoverDead can't see
				// it (no spool to walk). The heal runs under the commit lock
				// with state re-read there; only a SUCCESSFUL heal clears
				// the flag (a failed one must stay prune-protected).
				if spool.HealStranded(file) {
					inProgress = false
				}
			}
		}
		spans, _ := run["spans"].([]any)
		models := []string{}
		seen := map[string]bool{}
		for _, sv := range spans {
			s, _ := sv.(map[string]any)
			if s["type"] != "llm_call" {
				continue
			}
			model, _ := s["model"].(map[string]any)
			if id, ok := model["id"].(string); ok && id != "" && !seen[id] {
				seen[id] = true
				models = append(models, id)
			}
		}
		nameStr, _ := run["name"].(string)
		created, _ := run["created_at"].(string)
		out = append(out, Capture{
			File: file, Name: nameStr, CreatedAt: created,
			Spans: len(spans), Models: models, InProgress: inProgress,
		})
	}
	// Filenames embed the timestamp, so name order is time order (newest first).
	sort.Slice(out, func(i, j int) bool { return out[i].File > out[j].File })
	return out
}

/* ------------------------------------------------------ publish pipeline */

// Inspection mirrors inspectRunFile: read + validate + scan, never upload.
type Inspection struct {
	Text       string
	Name       string
	CreatedAt  string
	Fidelity   string // "exact" | "reconstructed" | ""
	Spans      int
	Models     []string
	Bytes      int
	InProgress bool
	Errors     []string
	Secrets    []scan.Hit
}

func InspectRunFile(file string) (*Inspection, error) {
	// A still-recording session may exist only as a spool — materialize.
	spool.Assemble(file, spool.AssembleOptions{})
	raw, err := os.ReadFile(file)
	if err != nil {
		return nil, fmt.Errorf("cannot read %s", file)
	}
	var anyData any
	if err := json.Unmarshal(raw, &anyData); err != nil {
		return nil, fmt.Errorf("%s is not valid JSON", file)
	}
	// Non-object JSON is valid JSON — the schema reports "must be object",
	// matching the JS error path.
	data, _ := anyData.(map[string]any)
	ins := &Inspection{Text: string(raw), Bytes: len(raw)}
	ins.Name, _ = data["name"].(string)
	ins.CreatedAt, _ = data["created_at"].(string)
	if source, ok := data["source"].(map[string]any); ok {
		ins.Fidelity, _ = source["fidelity"].(string)
	}
	meta, _ := data["metadata"].(map[string]any)
	ins.InProgress = meta != nil && meta["in_progress"] == true
	spans, _ := data["spans"].([]any)
	ins.Spans = len(spans)
	seen := map[string]bool{}
	for _, sv := range spans {
		s, _ := sv.(map[string]any)
		if s["type"] != "llm_call" {
			continue
		}
		model, _ := s["model"].(map[string]any)
		if id, ok := model["id"].(string); ok && id != "" && !seen[id] {
			seen[id] = true
			ins.Models = append(ins.Models, id)
		}
	}
	ins.Errors = format.ValidateRun(anyData)
	if len(ins.Errors) == 0 {
		ins.Secrets = scan.ForSecrets(ins.Text)
	}
	return ins, nil
}

// UploadResult mirrors uploadRun's return.
type UploadResult struct {
	OK     bool
	Status int
	Body   map[string]any
}

// UploadRun POSTs to the ingest API, gzipping anything over 64KB.
func UploadRun(text, server, apiKey string) UploadResult {
	body := []byte(text)
	headers := map[string]string{"content-type": "application/json"}
	if apiKey != "" {
		headers["authorization"] = "Bearer " + apiKey
	}
	if len(body) > 64*1024 {
		var buf bytes.Buffer
		zw := gzip.NewWriter(&buf)
		zw.Write(body)
		zw.Close()
		body = buf.Bytes()
		headers["content-encoding"] = "gzip"
	}
	req, err := http.NewRequest(http.MethodPost, server+"/api/runs", bytes.NewReader(body))
	if err != nil {
		return UploadResult{Body: errBody("unreachable", err.Error())}
	}
	for k, v := range headers {
		req.Header.Set(k, v)
	}
	client := &http.Client{Timeout: 5 * time.Minute}
	res, err := client.Do(req)
	if err != nil {
		return UploadResult{Body: errBody("unreachable", fmt.Sprintf("cannot reach %s: %v", server, err))}
	}
	defer res.Body.Close()
	var out map[string]any
	json.NewDecoder(res.Body).Decode(&out)
	return UploadResult{OK: res.StatusCode >= 200 && res.StatusCode < 300, Status: res.StatusCode, Body: out}
}

func errBody(code, message string) map[string]any {
	return map[string]any{"error": map[string]any{"code": code, "message": message}}
}

// DeleteRun tombstones a published session — the owner-only DELETE the
// ingest API already supports. Returns the HTTP status and a short body
// excerpt for error rendering (0 status = transport failure).
func DeleteRun(server, apiKey, id string) (int, string) {
	req, err := http.NewRequest(http.MethodDelete, server+"/api/runs/"+id, nil)
	if err != nil {
		return 0, err.Error()
	}
	req.Header.Set("authorization", "Bearer "+apiKey)
	client := &http.Client{Timeout: 30 * time.Second}
	res, err := client.Do(req)
	if err != nil {
		return 0, fmt.Sprintf("cannot reach %s: %v", server, err)
	}
	defer res.Body.Close()
	var out map[string]any
	json.NewDecoder(res.Body).Decode(&out)
	msg := ""
	if e, ok := out["error"].(map[string]any); ok {
		msg, _ = e["message"].(string)
	}
	return res.StatusCode, msg
}

/* ------------------------------------------------------------- retention */

// PlanPrune is the pure planner from cli/prune.mjs: remove what's empty
// (--empty), older than the window, or beyond keep; never in-progress.
func PlanPrune(captures []Capture, now time.Time, olderThan time.Duration, keep int, empty bool) (remove, kept []Capture) {
	for i, c := range captures {
		if c.InProgress {
			kept = append(kept, c)
			continue
		}
		tooOld := false
		if olderThan >= 0 { // 0 prunes everything finished, like the JS planner
			created, err := time.Parse(time.RFC3339, c.CreatedAt)
			tooOld = err == nil && now.Sub(created) > olderThan
		}
		isEmpty := c.Spans <= 1 // just the root agent span — nothing captured
		if (empty && isEmpty) || tooOld || (keep >= 0 && i >= keep) {
			remove = append(remove, c)
		} else {
			kept = append(kept, c)
		}
	}
	return remove, kept
}

// RemoveCapture deletes a capture and every sidecar the protocol defines.
func RemoveCapture(file string) {
	for _, p := range []string{
		file + ".spool", file + ".spool.pid", file + ".spool.corrupt", file + ".lock", file,
	} {
		os.Remove(p)
	}
}
