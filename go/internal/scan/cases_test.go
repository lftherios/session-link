package scan

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

// The cases every scanner is held to (testdata/format/README.md). The
// JavaScript test in test/format-cases.test.mjs and the Rust test in
// crates/slink-format read the same file.
func TestCases(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join("..", "..", "..", "testdata", "format", "scan.json"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		Name string `json:"name"`
		Text string `json:"text"`
		Hits []Hit  `json:"hits"`
	}
	if err := json.Unmarshal(raw, &cases); err != nil {
		t.Fatal(err)
	}
	if len(cases) == 0 {
		t.Fatal("no cases")
	}
	for _, c := range cases {
		t.Run(c.Name, func(t *testing.T) {
			if got := ForSecrets(c.Text); !reflect.DeepEqual(got, c.Hits) {
				t.Fatalf("want %+v, got %+v", c.Hits, got)
			}
		})
	}
}
