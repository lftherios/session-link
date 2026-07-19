// Package scan is the client-side pre-push credential scan — the same
// pattern list the TS server enforces at ingest, from the same generated
// artifact (secret-patterns.json, drift-checked by scripts/golden.mjs).
package scan

import (
	_ "embed"
	"encoding/json"
	"fmt"
	"regexp"
)

//go:embed secret-patterns.json
var patternsJSON []byte

type pattern struct {
	Name   string
	Source *regexp.Regexp
}

var patterns = mustLoad()

func mustLoad() []pattern {
	var raw []struct {
		Name   string `json:"name"`
		Source string `json:"source"`
	}
	if err := json.Unmarshal(patternsJSON, &raw); err != nil {
		panic(fmt.Sprintf("secret-patterns.json: %v", err))
	}
	out := make([]pattern, 0, len(raw))
	for _, p := range raw {
		// Every pattern is RE2-compatible by contract (see the artifact's
		// source header); a compile failure is a build-time bug.
		out = append(out, pattern{Name: p.Name, Source: regexp.MustCompile(p.Source)})
	}
	return out
}

// Hit mirrors the JS scanner's finding shape.
type Hit struct {
	Pattern string `json:"pattern"`
	Preview string `json:"preview"`
}

// ForSecrets mirrors scanForSecrets: per-pattern global matching, masked
// previews (first 8 chars + length), capped at 10 hits total.
func ForSecrets(text string) []Hit {
	hits := []Hit{}
	for _, p := range patterns {
		for _, m := range p.Source.FindAllString(text, -1) {
			head := m
			if len(head) > 8 {
				head = head[:8]
			}
			hits = append(hits, Hit{
				Pattern: p.Name,
				Preview: fmt.Sprintf("%s…(%d chars)", head, len(m)),
			})
			if len(hits) >= 10 {
				return hits
			}
		}
	}
	return hits
}
