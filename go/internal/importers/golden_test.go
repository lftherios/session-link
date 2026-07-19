package importers

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

// TestGoldenImports drives every registered importer over the same
// fixtures scripts/golden.mjs pins from the JS reference. Parity is
// parsed equality of the produced run.
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
				in := Input{Fallback: "golden"}
				if raw, err := os.ReadFile(filepath.Join(dir, "input.session.jsonl")); err == nil {
					for _, l := range strings.Split(string(raw), "\n") {
						if strings.TrimSpace(l) != "" {
							in.Lines = append(in.Lines, l)
						}
					}
				} else {
					readJSON(t, filepath.Join(dir, "input.session.json"), &in.Session)
					readJSON(t, filepath.Join(dir, "input.messages.json"), &in.Messages)
				}
				run, err := imp(in)
				if err != nil {
					t.Fatalf("import: %v", err)
				}
				var golden any
				readJSON(t, filepath.Join(dir, "golden.run.json"), &golden)
				got := roundTrip(t, run)
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
