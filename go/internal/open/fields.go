package open

import (
	"bytes"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json"
	"slices"

	"github.com/lftherios/session-link/internal/share"
)

// sessionPage is what the page around the reader needs from a session document.
type sessionPage struct {
	name       string
	spans      int
	inProgress bool
	excerpt    bool
	titleKey   string // where a title typed for the underlying session lives
	json       []byte // compact, ready to embed
}

// readSessionPage decodes as little as it can. handoff.Save names every
// preview by the SHA-256 of the compact bytes it validated, so a matching name
// proves the file is that document and only a few fields need reading. Any
// other file is decoded, checked and compacted in full.
func readSessionPage(id string, raw []byte, preview bool) (sessionPage, error) {
	if preview {
		if sum := sha256.Sum256(raw); hex.EncodeToString(sum[:]) == id {
			return scanSessionPage(raw), nil
		}
	}
	return decodeSessionPage(raw)
}

func decodeSessionPage(raw []byte) (sessionPage, error) {
	var run map[string]any
	if err := json.Unmarshal(raw, &run); err != nil {
		return sessionPage{}, errNotJSON
	}
	// The index greys non-session JSON as unrenderable — the direct URL
	// must agree, not serve a half-broken viewer over it.
	if schema, _ := run["schema"].(string); schema != "session/v0" && schema != "run/v0" {
		return sessionPage{}, errNotJSON
	}
	page := sessionPage{titleKey: runSessionTitleKey(run), json: mustCompact(raw)}
	page.name, _ = run["name"].(string)
	spans, _ := run["spans"].([]any)
	page.spans = len(spans)
	meta, _ := run["metadata"].(map[string]any)
	page.inProgress, _ = meta["in_progress"].(bool)
	extensions, _ := run["extensions"].(map[string]any)
	page.excerpt = extensions[share.Extension] != nil
	return page, nil
}

// scanSessionPage reads the same facts as decodeSessionPage from a document
// already known to be valid, without decoding its spans.
func scanSessionPage(raw []byte) sessionPage {
	top := objectFields(raw, "name", "spans", "metadata", "source", "extensions")
	meta := objectFields(top["metadata"], "in_progress", "session_id")
	page := sessionPage{spans: countElements(top["spans"]), json: raw}
	var harness, session string
	json.Unmarshal(top["name"], &page.name)
	json.Unmarshal(meta["in_progress"], &page.inProgress)
	json.Unmarshal(objectFields(top["source"], "harness")["harness"], &harness)
	json.Unmarshal(meta["session_id"], &session)
	page.titleKey = sessionTitleKey(harness, session)
	value, ok := objectFields(top["extensions"], share.Extension)[share.Extension]
	page.excerpt = ok && string(value) != "null"
	return page
}

// objectFields returns the raw values of the named members of a JSON object
// without decoding anything else. data must be valid JSON, and keys are
// compared as written. Strings are skipped with a memory search for their
// closing quote, so megabytes of tool output cost almost nothing.
func objectFields(data []byte, names ...string) map[string][]byte {
	fields := map[string][]byte{}
	i := skipSpace(data, 0)
	if i >= len(data) || data[i] != '{' {
		return fields
	}
	for i = skipSpace(data, i+1); i < len(data) && data[i] == '"'; i = skipSpace(data, i) {
		end := skipString(data, i)
		key := string(data[i+1 : max(i+1, end-1)])
		if i = skipSpace(data, end); i >= len(data) || data[i] != ':' {
			break
		}
		start := skipSpace(data, i+1)
		i = skipValue(data, start)
		if slices.Contains(names, key) {
			fields[key] = data[start:i]
		}
		if i = skipSpace(data, i); i < len(data) && data[i] == ',' {
			i++
		}
	}
	return fields
}

// countElements returns how many values the JSON array in data holds.
func countElements(data []byte) int {
	i := skipSpace(data, 0)
	if i >= len(data) || data[i] != '[' {
		return 0
	}
	count := 0
	for i = skipSpace(data, i+1); i < len(data) && data[i] != ']'; i = skipSpace(data, i) {
		next := skipValue(data, i)
		if next == i {
			break // malformed; never loop without progress
		}
		count++
		if i = skipSpace(data, next); i < len(data) && data[i] == ',' {
			i++
		}
	}
	return count
}

// skipValue returns the index just past the JSON value that starts at data[i].
func skipValue(data []byte, i int) int {
	depth := 0
	for i < len(data) {
		c := data[i]
		switch {
		case c == '"':
			i = skipString(data, i)
		case c == '{' || c == '[':
			depth++
			i++
		case c == '}' || c == ']':
			if depth == 0 {
				return i // a number or literal ended at its parent's close
			}
			depth--
			i++
		case depth == 0 && (c == ',' || c == ' ' || c == '\t' || c == '\n' || c == '\r'):
			return i // a number or literal ended
		default:
			i++
		}
		if depth == 0 && (c == '"' || c == '}' || c == ']') {
			return i
		}
	}
	return i
}

// skipString returns the index just past the string that opens at data[i].
// A quote closes the string unless an odd run of backslashes precedes it.
func skipString(data []byte, i int) int {
	for j := i + 1; ; {
		n := bytes.IndexByte(data[j:], '"')
		if n < 0 {
			return len(data)
		}
		j += n + 1
		escapes := 0
		for k := j - 2; k > i && data[k] == '\\'; k-- {
			escapes++
		}
		if escapes%2 == 0 {
			return j
		}
	}
}

func skipSpace(data []byte, i int) int {
	for i < len(data) && (data[i] == ' ' || data[i] == '\t' || data[i] == '\n' || data[i] == '\r') {
		i++
	}
	return i
}
