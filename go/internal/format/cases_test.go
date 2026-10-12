package format

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
)

// formatCase is one entry of testdata/format/cases.json, the cases every
// validator is held to (testdata/format/README.md). The JavaScript test in
// test/format-cases.test.mjs and the Rust test in crates/slink-format read
// the same file.
type formatCase struct {
	Name   string          `json:"name"`
	Doc    json.RawMessage `json:"doc"`
	Text   *string         `json:"text"`
	Expect json.RawMessage `json:"expect"`
	At     string          `json:"at"`
	Error  string          `json:"error"`
	Count  int             `json:"count"`
}

func TestCases(t *testing.T) {
	raw, err := os.ReadFile(filepath.Join("..", "..", "..", "testdata", "format", "cases.json"))
	if err != nil {
		t.Fatal(err)
	}
	var cases []formatCase
	if err := json.Unmarshal(raw, &cases); err != nil {
		t.Fatal(err)
	}
	if len(cases) == 0 {
		t.Fatal("no cases")
	}
	for _, c := range cases {
		t.Run(c.Name, func(t *testing.T) {
			// One verdict for every implementation, or one for each.
			var expected string
			if json.Unmarshal(c.Expect, &expected) != nil {
				var each map[string]string
				if err := json.Unmarshal(c.Expect, &each); err != nil {
					t.Fatal(err)
				}
				expected = each["go"]
			}
			text := []byte(c.Doc)
			if c.Text != nil {
				text = []byte(*c.Text)
			}
			// Parsed as cli.InspectRunFile parses a capture.
			var data any
			if err := json.Unmarshal(text, &data); err != nil {
				if expected != "unreadable" {
					t.Fatalf("want %s, but the text does not parse: %v", expected, err)
				}
				return
			}
			errs := ValidateRun(data)
			got := "valid"
			if len(errs) > 0 {
				got = "invalid"
			}
			if got != expected {
				t.Fatalf("want %s, got %s: %v", expected, got, errs)
			}
			if c.At != "" {
				found := false
				for _, e := range errs {
					found = found || strings.HasPrefix(e, c.At+" ")
				}
				if !found {
					t.Fatalf("no error at %s: %v", c.At, errs)
				}
			}
			if c.Error != "" && !reflect.DeepEqual(errs, []string{c.Error}) {
				t.Fatalf("want [%s], got %v", c.Error, errs)
			}
			if c.Count != 0 && len(errs) != c.Count {
				t.Fatalf("want %d errors, got %d", c.Count, len(errs))
			}
		})
	}
}
