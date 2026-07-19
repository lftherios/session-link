// Package importers ports the five harness importers — the mapping layer
// from each coding agent's on-disk history to session/v0. Each importer
// lives in its own file and registers itself in init(); parity with the JS
// reference is enforced by the testdata/import goldens (parsed equality),
// exactly like normalize.
package importers

// Importer builds a run from a harness's parsed inputs. File-based
// harnesses (claude-code, pi, codex) receive transcript lines in Lines;
// row-based ones (opencode, hermes) receive Session and Messages.
type Input struct {
	Lines    []string
	Session  map[string]any
	Messages []any
	Fallback string // fallback session name
}

type Importer func(Input) (map[string]any, error)

// Registry: filled by each importer file's init().
var Registry = map[string]Importer{}

/* ------------------------------------------------- shared any-tree utils */

func m(v any) map[string]any {
	mm, _ := v.(map[string]any)
	return mm
}

func arr(v any) []any {
	a, _ := v.([]any)
	return a
}

func str(v any) (string, bool) {
	s, ok := v.(string)
	return s, ok
}

func strOr(v any, def string) string {
	if s, ok := v.(string); ok {
		return s
	}
	return def
}

func numOr(v any, def float64) float64 {
	if f, ok := v.(float64); ok {
		return f
	}
	return def
}
