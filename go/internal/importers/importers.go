// Package importers holds the harness importers — the mapping layer
// from each coding agent's on-disk history to session/v0. Each importer
// lives in its own file and registers itself in init(); the output each must
// produce is pinned by the testdata/import goldens (parsed equality).
package importers

import (
	"bytes"
	"encoding/base64"
	"fmt"
	"strings"
	"time"
)

// Importer builds a run from a harness's parsed inputs. File-based
// harnesses (claude-code, pi, omp, codex) receive transcript lines in Lines;
// row-based ones (opencode, hermes) receive Session and Messages.
type Input struct {
	Lines    []string
	Session  map[string]any
	Messages []any
	Fallback string // fallback session name
	// Blobs is the directory a harness keeps a transcript's large content in,
	// beside the transcript, when it has one. omp moves images there.
	Blobs string
	// Agents are the sub-agents this session started. A harness records each
	// one's work apart from the session: in a file beside it, in another
	// rollout, or as another row.
	Agents []Agent
	// SubAgent marks the transcript of a sub-agent, read to be nested in the
	// session that started it.
	SubAgent bool
}

// Agent is one sub-agent's session as its harness recorded it.
type Agent struct {
	ID    string // what the parent's record calls it
	Call  string // the tool call that started it, when the harness says so itself
	Name  string
	Input Input
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

/* -------------------------------------------------------------- images
   A harness records an image as base64, as a data URL or as a link. The
   session/v0 part carries a URL either way; the viewer shows a data URL as it
   is and never fetches a link without a click. */

// imagePart is the part for an image recorded as base64. mime is the
// harness's own label; where it gave none, the image's first bytes say. An
// image whose kind cannot be told is not turned into a part.
func imagePart(data, mime string) (map[string]any, bool) {
	if data == "" {
		return nil, false
	}
	if mime == "" {
		head := data
		if len(head) > 24 {
			head = head[:24]
		}
		raw, _ := base64.StdEncoding.DecodeString(head)
		switch {
		case bytes.HasPrefix(raw, []byte("\x89PNG")):
			mime = "image/png"
		case bytes.HasPrefix(raw, []byte("\xff\xd8\xff")):
			mime = "image/jpeg"
		case bytes.HasPrefix(raw, []byte("GIF8")):
			mime = "image/gif"
		case len(raw) >= 12 && string(raw[:4]) == "RIFF" && string(raw[8:12]) == "WEBP":
			mime = "image/webp"
		default:
			return nil, false
		}
	}
	return map[string]any{"type": "image", "url": "data:" + mime + ";base64," + data, "mime": mime}, true
}

// imageURLPart is the part for an image recorded as a URL: a data URL holding
// the image itself, or a link.
func imageURLPart(url string) (map[string]any, bool) {
	if rest, ok := strings.CutPrefix(url, "data:"); ok {
		mime, _, found := strings.Cut(rest, ";")
		if !found || !strings.HasPrefix(mime, "image/") {
			return nil, false
		}
		return map[string]any{"type": "image", "url": url, "mime": mime}, true
	}
	if strings.HasPrefix(url, "http://") || strings.HasPrefix(url, "https://") {
		return map[string]any{"type": "image", "url": url}, true
	}
	return nil, false
}

/* ---------------------------------------------------------- sub-agents */

// adoptAgents nests each sub-agent's session in run, as a child agent under
// the tool call that started it. The harness names that call itself (Call) or
// the importer learned it while reading the parent (calls, by the agent's
// ID); a sub-agent with neither goes under the tool call that was running
// when it started, and failing that under the session. An agent's spans
// follow the call they belong to, so the session reads in the order it ran.
func adoptAgents(run map[string]any, harness string, agents []Agent, calls map[string]string) {
	importer := Registry[harness]
	own := arr(run["spans"])
	callSpan := map[string]string{} // tool call id -> the span of that call
	note := func(spans []any) {
		for _, sv := range spans {
			span := m(sv)
			if id := strOr(m(span["input"])["tool_call_id"], ""); strOr(span["type"], "") == "tool_call" && id != "" && callSpan[id] == "" {
				callSpan[id] = strOr(span["id"], "")
			}
		}
	}
	note(own)
	under := map[string][][]any{} // span id -> the agents that span started
	adopted := 0
	for _, agent := range agents {
		agent.Input.SubAgent = true
		child, err := importer(agent.Input)
		childSpans := arr(child["spans"])
		if err != nil || len(childSpans) < 2 {
			continue // nothing was recorded for it beyond its start
		}
		adopted++
		prefix := fmt.Sprintf("a%d", adopted)
		childRoot := m(childSpans[0])
		call := agent.Call
		if call == "" {
			call = calls[agent.ID]
		}
		anchor := callSpan[call]
		if started := spanTime(childRoot["started_at"]); anchor == "" && !started.IsZero() {
			for _, sv := range own {
				span := m(sv)
				from, to := spanTime(span["started_at"]), spanTime(span["ended_at"])
				if strOr(span["type"], "") == "tool_call" && !from.IsZero() && !started.Before(from) && !started.After(to) {
					anchor = strOr(span["id"], "")
				}
			}
		}
		if anchor == "" {
			anchor = "root"
		}
		// Named by its harness, else after the call that started it.
		name := agent.Name
		for _, sv := range own {
			if span := m(sv); name == "" && strOr(span["id"], "") == anchor && anchor != "root" {
				name = strOr(span["name"], "")
			}
		}
		if name == "" {
			name = strOr(childRoot["name"], agent.ID)
		}
		head := map[string]any{"id": prefix, "parent_id": anchor, "type": "agent", "name": name}
		for _, key := range []string{"started_at", "ended_at", "status"} {
			if v, ok := childRoot[key]; ok {
				head[key] = v
			}
		}
		block := []any{head}
		for _, sv := range childSpans[1:] {
			span := m(sv)
			span["id"] = prefix + "." + strOr(span["id"], "")
			if parent := strOr(span["parent_id"], "root"); parent == "root" {
				span["parent_id"] = prefix
			} else {
				span["parent_id"] = prefix + "." + parent
			}
			block = append(block, span)
		}
		note(block)
		under[anchor] = append(under[anchor], block)
	}
	if adopted == 0 {
		return
	}
	var ordered []any
	var place func(spans []any)
	place = func(spans []any) {
		for _, sv := range spans {
			ordered = append(ordered, sv)
			if id := strOr(m(sv)["id"], ""); id != "root" {
				for _, block := range under[id] {
					place(block)
				}
			}
		}
	}
	place(own)
	for _, block := range under["root"] {
		place(block)
	}
	run["spans"] = ordered
	m(run["metadata"])["sub_agents"] = adopted
}

func spanTime(v any) time.Time {
	t, _ := time.Parse(time.RFC3339Nano, strOr(v, ""))
	return t
}

func numOr(v any, def float64) float64 {
	if f, ok := v.(float64); ok {
		return f
	}
	return def
}
