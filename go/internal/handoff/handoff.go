// Package handoff resolves explicit harness sessions and saves independent
// local previews. Nothing in this package uploads data or requires an account.
package handoff

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/lftherios/session-link/internal/format"
	"github.com/lftherios/session-link/internal/importers"
	"github.com/lftherios/session-link/internal/spool"
)

type Source struct {
	ID, Title, Harness, Dir string
	// Name is a title the harness recorded; Prompt is the first real prompt.
	Name, Prompt     string
	Updated, Started time.Time
	Read             func() ([]byte, error)
}

func Native(candidate importers.Candidate) Source {
	return Source{ID: candidate.ID, Title: candidate.Title, Harness: candidate.Harness, Name: candidate.Name, Prompt: candidate.Prompt, Started: candidate.Started,
		Dir: candidate.Dir, Updated: time.Unix(0, candidate.Recency), Read: func() ([]byte, error) {
			in, err := candidate.Load()
			if err != nil {
				return nil, err
			}
			return importDocument(candidate.Harness, in)
		}}
}

func importDocument(harness string, in importers.Input) ([]byte, error) {
	build := importers.Registry[harness]
	if build == nil {
		return nil, fmt.Errorf("unknown agent %q", harness)
	}
	run, err := build(in)
	if err != nil {
		return nil, err
	}
	spans, _ := run["spans"].([]any)
	if len(spans) == 0 {
		return nil, fmt.Errorf("this %s session has no messages yet", harness)
	}
	return json.Marshal(run)
}

// File accepts a capture or a harness transcript, including files outside the
// capture directory. The saved preview is what the web server will serve.
func File(file, harness string) (Source, error) {
	abs, err := filepath.Abs(file)
	if err != nil {
		return Source{}, err
	}
	info, err := os.Stat(abs)
	if os.IsNotExist(err) {
		info, err = os.Stat(spool.SpoolPath(abs))
	}
	if err != nil || !info.Mode().IsRegular() {
		return Source{}, fmt.Errorf("cannot read session %q", file)
	}
	if harness != "" && importers.Registry[harness] == nil {
		return Source{}, fmt.Errorf("unknown agent %q", harness)
	}
	return Source{ID: filepath.Base(abs), Title: filepath.Base(abs), Harness: harness, Updated: info.ModTime(), Read: func() ([]byte, error) {
		if _, err := os.Stat(spool.SpoolPath(abs)); err == nil {
			if _, err := spool.Assemble(abs, spool.AssembleOptions{}); err != nil {
				return nil, err
			}
		}
		data, err := os.ReadFile(abs)
		if err != nil {
			return nil, err
		}
		var doc map[string]any
		if json.Unmarshal(data, &doc) == nil && (doc["schema"] == "session/v0" || doc["schema"] == "run/v0") {
			return data, nil
		}
		in, detected, err := importers.LoadFile(abs)
		if err != nil {
			return nil, err
		}
		if harness != "" {
			detected = harness
		}
		if detected == "" {
			return nil, fmt.Errorf("cannot identify the agent for %q; pass --from", file)
		}
		return importDocument(detected, in)
	}}, nil
}

// Save uses content-addressed files in a separate preview directory. Capture
// recovery and an agent continuing to write cannot change a published preview.
func Save(dir string, source Source) (string, error) {
	data, err := source.Read()
	if err != nil {
		return "", err
	}
	var doc map[string]any
	if err := json.Unmarshal(data, &doc); err != nil {
		return "", fmt.Errorf("cannot read session JSON: %w", err)
	}
	// Validate the legacy spelling against the same structural contract while
	// preserving the original bytes in the preview.
	schema := doc["schema"]
	if schema == "run/v0" {
		doc["schema"] = "session/v0"
	}
	if issues := format.ValidateRun(doc); len(issues) != 0 {
		return "", fmt.Errorf("cannot preview this session: %s", strings.Join(issues, "; "))
	}
	var compact bytes.Buffer
	if err := json.Compact(&compact, data); err != nil {
		return "", err
	}
	data = compact.Bytes()
	hash := sha256.Sum256(data)
	id := hex.EncodeToString(hash[:])
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", err
	}
	file := filepath.Join(dir, id+".json")
	if existing, err := os.ReadFile(file); err == nil && bytes.Equal(existing, data) {
		return id, nil
	}
	// Rename a fully written file so concurrent readers never see a partial
	// document. Equal hashes have equal bytes, so concurrent saves agree.
	tmp, err := os.CreateTemp(dir, ".preview-*")
	if err != nil {
		return "", err
	}
	defer os.Remove(tmp.Name())
	if _, err = tmp.Write(data); err != nil {
		tmp.Close()
		return "", err
	}
	if err = tmp.Close(); err != nil {
		return "", err
	}
	if err = os.Rename(tmp.Name(), file); err != nil {
		return "", err
	}
	return id, nil
}
