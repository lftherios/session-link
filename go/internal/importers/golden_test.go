package importers

import (
	"bytes"
	"encoding/json"
	"flag"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"

	"github.com/lftherios/session-link/internal/format"
)

// update rewrites each golden.run.json that no longer matches what its
// importer produces:
//
//	go test ./internal/importers -run TestGoldenImports -update
//
// The goldens are the expected behaviour, so read the diff before keeping it.
var update = flag.Bool("update", false, "rewrite the import goldens from the importers' output")

// TestGoldenImports drives every registered importer over the fixtures in
// testdata/import. A fixture passes on parsed equality with its golden run.
func TestGoldenImports(t *testing.T) {
	wd, _ := os.Getwd()
	base := filepath.Join(wd, "..", "..", "..", "testdata", "import")
	harnesses, err := os.ReadDir(base)
	if err != nil {
		t.Fatalf("no import fixtures: %v", err)
	}
	ran := 0
	for _, h := range harnesses {
		if !h.IsDir() {
			continue
		}
		harness := h.Name()
		imp := Registry[harness]
		cases, _ := os.ReadDir(filepath.Join(base, harness))
		for _, c := range cases {
			if !c.IsDir() {
				continue
			}
			ran++
			t.Run(harness+"/"+c.Name(), func(t *testing.T) {
				if imp == nil {
					t.Fatalf("no Go importer registered for %q", harness)
				}
				dir := filepath.Join(base, harness, c.Name())
				// A fixture can carry the blob store its transcript refers to.
				in := Input{Fallback: "golden", Blobs: filepath.Join(dir, "blobs")}
				if raw, err := os.ReadFile(filepath.Join(dir, "input.session.jsonl")); err == nil {
					for _, l := range strings.Split(string(raw), "\n") {
						if strings.TrimSpace(l) != "" {
							in.Lines = append(in.Lines, l)
						}
					}
					// A sub-agent's transcript sits by the fixture's own the way
					// its harness lays it out, and is read by the same code.
					in.Agents = fileAgents(harness, filepath.Join(dir, "input.session.jsonl"))
				} else {
					readJSON(t, filepath.Join(dir, "input.session.json"), &in.Session)
					readJSON(t, filepath.Join(dir, "input.messages.json"), &in.Messages)
					var agents []fixtureAgent
					if _, err := os.Stat(filepath.Join(dir, "input.agents.json")); err == nil {
						readJSON(t, filepath.Join(dir, "input.agents.json"), &agents)
					}
					in.Agents = fixtureAgents(agents)
				}
				run, err := imp(in)
				if err != nil {
					t.Fatalf("import: %v", err)
				}
				if issues := format.ValidateRun(run); len(issues) != 0 {
					t.Fatalf("the imported run is not valid session/v0: %v", issues)
				}
				goldenFile := filepath.Join(dir, "golden.run.json")
				got := roundTrip(t, run)
				if *update {
					// A golden that still matches keeps its bytes.
					var current any
					if raw, err := os.ReadFile(goldenFile); err == nil && json.Unmarshal(raw, &current) == nil && reflect.DeepEqual(got, current) {
						return
					}
					var out bytes.Buffer
					enc := json.NewEncoder(&out)
					enc.SetEscapeHTML(false)
					enc.SetIndent("", "  ")
					if err := enc.Encode(run); err != nil {
						t.Fatal(err)
					}
					if err := os.WriteFile(goldenFile, out.Bytes(), 0o644); err != nil {
						t.Fatal(err)
					}
					return
				}
				var golden any
				readJSON(t, goldenFile, &golden)
				if !reflect.DeepEqual(got, golden) {
					g, _ := json.MarshalIndent(got, "", "  ")
					w, _ := json.MarshalIndent(golden, "", "  ")
					t.Fatalf("run mismatch\n--- got ---\n%s\n--- want ---\n%s", g, w)
				}
			})
		}
	}
	if ran == 0 {
		t.Fatal("zero import fixtures ran")
	}
}

// fixtureAgent is a sub-agent's session in a fixture for a harness that keeps
// sessions as rows: what its loader hands the importer for a child session.
type fixtureAgent struct {
	ID       string         `json:"id"`
	Name     string         `json:"name"`
	Session  map[string]any `json:"session"`
	Messages []any          `json:"messages"`
	Agents   []fixtureAgent `json:"agents,omitempty"`
}

func fixtureAgents(recorded []fixtureAgent) []Agent {
	var agents []Agent
	for _, a := range recorded {
		agents = append(agents, Agent{ID: a.ID, Name: a.Name, Input: Input{Session: a.Session, Messages: a.Messages, Agents: fixtureAgents(a.Agents)}})
	}
	return agents
}

func readJSON(t *testing.T, path string, v any) {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(b, v); err != nil {
		t.Fatalf("%s: %v", path, err)
	}
}

func roundTrip(t *testing.T, v any) any {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	var out any
	if err := json.Unmarshal(b, &out); err != nil {
		t.Fatal(err)
	}
	return out
}
