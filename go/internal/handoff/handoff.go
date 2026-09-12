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
	// Files lists every file Read depends on. While none of them changes size
	// or modification time, Save reuses the snapshot it made from them.
	Files []string
}

// transcriptFiles tracks a transcript's one file. Sessions stored in a
// database have none, so they are imported on every open.
func transcriptFiles(candidate importers.Candidate) []string {
	if candidate.File == "" {
		return nil
	}
	return []string{candidate.File}
}

func Native(candidate importers.Candidate) Source {
	return Source{ID: candidate.ID, Title: candidate.Title, Harness: candidate.Harness, Name: candidate.Name, Prompt: candidate.Prompt, Started: candidate.Started,
		Dir: candidate.Dir, Updated: time.Unix(0, candidate.Recency), Files: transcriptFiles(candidate), Read: func() ([]byte, error) {
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
	// Stamp before reading: a file that changes during the import only costs
	// another import later, never a stale snapshot.
	stamp := sourceStamp(source)
	if id := reusableSnapshot(dir, source, stamp); id != "" {
		return id, nil
	}
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
	if existing, err := os.ReadFile(file); err != nil || !bytes.Equal(existing, data) {
		// Equal hashes have equal bytes, so concurrent saves agree.
		if err := writeFile(file, data); err != nil {
			return "", err
		}
	}
	rememberSnapshot(dir, source, stamp, id)
	return id, nil
}

// writeFile renames a fully written file into place so concurrent readers
// never see a partial document.
func writeFile(file string, data []byte) error {
	tmp, err := os.CreateTemp(filepath.Dir(file), ".preview-*")
	if err != nil {
		return err
	}
	defer os.Remove(tmp.Name())
	if _, err = tmp.Write(data); err != nil {
		tmp.Close()
		return err
	}
	if err = tmp.Close(); err != nil {
		return err
	}
	return os.Rename(tmp.Name(), file)
}

// buildStamp identifies this slink binary. It is taken at startup, so a
// snapshot an older importer made is never reused after an upgrade.
var buildStamp = executableStamp()

func executableStamp() string {
	exe, err := os.Executable()
	if err != nil {
		return ""
	}
	info, err := os.Stat(exe)
	if err != nil {
		return ""
	}
	return fmt.Sprintf("%s %d %d", exe, info.Size(), info.ModTime().UnixNano())
}

// sourceStamp describes what a snapshot is made from: this build, the
// harness, and each source file's size and modification time. Nil means the
// snapshot cannot be reused.
func sourceStamp(source Source) []byte {
	if len(source.Files) == 0 || buildStamp == "" {
		return nil
	}
	type fileStamp struct {
		Path     string `json:"path"`
		Size     int64  `json:"size"`
		Modified int64  `json:"modified_ns"`
	}
	files := make([]fileStamp, len(source.Files))
	for i, path := range source.Files {
		info, err := os.Stat(path)
		if err != nil {
			return nil
		}
		files[i] = fileStamp{Path: path, Size: info.Size(), Modified: info.ModTime().UnixNano()}
	}
	stamp, err := json.Marshal(map[string]any{"build": buildStamp, "harness": source.Harness, "files": files})
	if err != nil {
		return nil
	}
	return stamp
}

type snapshotRecord struct {
	Stamp    json.RawMessage `json:"stamp"`
	Snapshot string          `json:"snapshot"`
}

// recordPath keeps one record per source, replaced whenever it changes.
func recordPath(dir string, source Source) string {
	sum := sha256.Sum256([]byte(source.Harness + "\n" + strings.Join(source.Files, "\n")))
	return filepath.Join(dir, "sources", hex.EncodeToString(sum[:16])+".json")
}

// reusableSnapshot returns the snapshot made from exactly this stamp, or ""
// when anything changed. The snapshot's bytes must still match its name, so
// a damaged file is rebuilt rather than served.
func reusableSnapshot(dir string, source Source, stamp []byte) string {
	if stamp == nil {
		return ""
	}
	raw, err := os.ReadFile(recordPath(dir, source))
	if err != nil {
		return ""
	}
	var record snapshotRecord
	if json.Unmarshal(raw, &record) != nil || !bytes.Equal(record.Stamp, stamp) {
		return ""
	}
	data, err := os.ReadFile(filepath.Join(dir, filepath.Base(record.Snapshot)+".json"))
	if err != nil {
		return ""
	}
	if sum := sha256.Sum256(data); hex.EncodeToString(sum[:]) != record.Snapshot {
		return ""
	}
	return record.Snapshot
}

// rememberSnapshot is best effort: without a record the next open imports again.
func rememberSnapshot(dir string, source Source, stamp []byte, id string) {
	if stamp == nil {
		return
	}
	record, err := json.Marshal(snapshotRecord{Stamp: stamp, Snapshot: id})
	path := recordPath(dir, source)
	if err == nil && os.MkdirAll(filepath.Dir(path), 0o700) == nil {
		writeFile(path, record)
	}
}
