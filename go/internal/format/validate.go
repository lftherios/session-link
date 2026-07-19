// Package format validates session/v0 documents: the JSON Schema (embedded
// from the drift-checked copy of packages/format/session.schema.json) plus
// the invariants the schema can't express — the same rules validate.mjs
// enforces for the server and JS CLI.
package format

import (
	"bytes"
	_ "embed"
	"fmt"

	"github.com/santhosh-tekuri/jsonschema/v6"
)

//go:embed session.schema.json
var schemaJSON []byte

var compiled = mustCompile()

func mustCompile() *jsonschema.Schema {
	doc, err := jsonschema.UnmarshalJSON(bytes.NewReader(schemaJSON))
	if err != nil {
		panic(err)
	}
	c := jsonschema.NewCompiler()
	if err := c.AddResource("session.schema.json", doc); err != nil {
		panic(err)
	}
	s, err := c.Compile("session.schema.json")
	if err != nil {
		panic(err)
	}
	return s
}

// ValidateRun mirrors validateRun: schema errors first (capped), then the
// structural invariants (unique span ids, acyclic parent chains).
func ValidateRun(data any) []string {
	if err := compiled.Validate(data); err != nil {
		if ve, ok := err.(*jsonschema.ValidationError); ok {
			errs := []string{}
			for _, cause := range flatten(ve) {
				errs = append(errs, cause)
				if len(errs) >= 20 {
					break
				}
			}
			return errs
		}
		return []string{err.Error()}
	}
	return invariantErrors(data)
}

func flatten(ve *jsonschema.ValidationError) []string {
	if len(ve.Causes) == 0 {
		loc := ve.InstanceLocation
		path := "/"
		if len(loc) > 0 {
			path = ""
			for _, p := range loc {
				path += "/" + p
			}
		}
		return []string{fmt.Sprintf("%s %s", path, ve.ErrorKind.LocalizedString(nil))}
	}
	out := []string{}
	for _, c := range ve.Causes {
		out = append(out, flatten(c)...)
	}
	return out
}

func invariantErrors(data any) []string {
	run, _ := data.(map[string]any)
	spans, _ := run["spans"].([]any)
	byID := map[string]map[string]any{}
	for _, sv := range spans {
		s, _ := sv.(map[string]any)
		id, _ := s["id"].(string)
		if _, dup := byID[id]; dup {
			return []string{fmt.Sprintf("duplicate span id %q", id)}
		}
		byID[id] = s
	}
	acyclic := map[string]bool{}
	for _, sv := range spans {
		s, _ := sv.(map[string]any)
		seen := map[string]bool{}
		cur := s
		for cur != nil {
			id, _ := cur["id"].(string)
			if acyclic[id] {
				break
			}
			if seen[id] {
				return []string{fmt.Sprintf("parent_id cycle involving span %q", id)}
			}
			seen[id] = true
			parent, _ := cur["parent_id"].(string)
			if parent == "" {
				break
			}
			cur = byID[parent]
		}
		for id := range seen {
			acyclic[id] = true
		}
	}
	return []string{}
}
