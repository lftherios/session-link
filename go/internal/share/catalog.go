// Package share builds explicit, text-based excerpts from a saved session.
// It never copies a source envelope: only selected, visible content is exported.
package share

import (
	"encoding/json"
	"fmt"
	"strings"
)

const Extension = "session_link.share.v1"

type Unit struct {
	ID               string   `json:"id"`
	SpanID           string   `json:"span_id"`
	Role             string   `json:"role"`
	Kind             string   `json:"kind"`
	Text             string   `json:"text"`
	PromptIDs        []string `json:"prompt_ids,omitempty"`
	Unavailable      bool     `json:"unavailable,omitempty"`
	PromptIncomplete bool     `json:"prompt_incomplete,omitempty"`
	// Only used locally to detect missing tool context. Never copied to exports.
	CallID string `json:"-"`
	Scope  string `json:"-"`
}

type Catalog struct {
	Units []Unit `json:"units"`
}

func object(v any) map[string]any { m, _ := v.(map[string]any); return m }
func array(v any) []any           { a, _ := v.([]any); return a }
func str(v any) string            { s, _ := v.(string); return s }
func canonical(v any) string      { b, _ := json.Marshal(v); return string(b) }
func display(v any) string {
	if s, ok := v.(string); ok {
		return s
	}
	b, _ := json.MarshalIndent(v, "", "  ")
	return string(b)
}
func role(v any) string {
	s := str(v)
	switch s {
	case "user", "assistant", "tool", "system", "developer":
		return s
	}
	return "other"
}

// BuildCatalog collapses complete replayed histories, scoped to each agent.
// Deltas, including repeated user prompts, remain separate selections. Tool
// evidence is read both from messages and standalone tool spans.
func BuildCatalog(run map[string]any) Catalog {
	cat := Catalog{Units: []Unit{}}
	spans := array(run["spans"])
	byID := map[string]map[string]any{}
	for _, value := range spans {
		s := object(value)
		byID[str(s["id"])] = s
	}
	scopeOf := func(s map[string]any) string {
		p := str(s["parent_id"])
		seen := map[string]bool{}
		for p != "" && !seen[p] {
			seen[p] = true
			parent := byID[p]
			if parent["type"] == "agent" {
				return p
			}
			p = str(parent["parent_id"])
		}
		return ""
	}
	source := object(run["source"])
	legacyCodex := source["kind"] == "import" && source["harness"] == "codex"
	delta := source["kind"] == "import" || (source["kind"] == "sdk" && strings.HasPrefix(str(source["label"]), "pi-extension@"))
	history := map[string][]string{}
	prompts := map[string][]string{}
	promptGaps := map[string]bool{}
	systems := map[string]string{}
	seenTools := map[string]bool{}
	for si, value := range spans {
		s := object(value)
		sid, scope := str(s["id"]), scopeOf(s)
		add := func(id, r, kind, text, call string, unavailable bool) {
			if text == "" && !unavailable {
				return
			}
			if call != "" && kind == "tool_call" {
				key := canonical([]string{scope, kind, call, text})
				if seenTools[key] {
					return
				}
				seenTools[key] = true
			}
			u := Unit{ID: id, SpanID: sid, Role: r, Kind: kind, Text: text, CallID: call, Scope: scope, Unavailable: unavailable}
			if r != "user" {
				u.PromptIDs = append([]string{}, prompts[scope]...)
				u.PromptIncomplete = promptGaps[scope]
			}
			cat.Units = append(cat.Units, u)
		}
		var parts func([]any, string, string, string, string)
		resultParts := func(items []any, base, call string) {
			if call != "" {
				key := canonical([]any{scope, "tool_result", call, items})
				if seenTools[key] {
					return
				}
				seenTools[key] = true
			}
			parts(items, base, "tool", "tool_result", call)
		}
		parts = func(items []any, base, r, kind, call string) {
			for pi, item := range items {
				p := object(item)
				id := fmt.Sprintf("%s-%d", base, pi)
				switch p["type"] {
				case "text":
					add(id, r, kind, str(p["text"]), call, false)
					if r != "user" && kind == "text" {
						for di, definition := range definitionBlocks(str(p["text"])) {
							add(fmt.Sprintf("%s-ref-%d", id, di), r, "source_reference", definition, "", false)
						}
					}
				case "thinking":
					text := str(p["text"])
					unavailable := p["unavailable"] == true || strings.TrimSpace(text) == "" || (legacyCodex && strings.TrimSpace(text) == "[reasoning]")
					if unavailable {
						text = "Reasoning text isn't available in this capture."
					}
					add(id, r, "thinking", text, "", unavailable)
				case "tool_call":
					add(id, "assistant", "tool_call", display(map[string]any{"name": p["name"], "arguments": p["arguments"]}), str(p["id"]), false)
				case "tool_result":
					resultParts(array(p["content"]), id, str(p["tool_call_id"]))
				case "data":
					add(id, r, "data", display(p["data"]), "", false)
				default:
					add(id, r, "unavailable", "This image, attachment, or unrecognized content cannot be included in a text excerpt yet.", "", true)
				}
			}
		}
		input, output := object(s["input"]), object(s["output"])
		if system := str(input["system"]); system != "" && systems[scope] != system {
			add(fmt.Sprintf("u%d-system", si), "system", "text", system, "", false)
			systems[scope] = system
		}
		for _, field := range []string{"system_ref", "messages_ref", "tools_ref"} {
			if input[field] != nil {
				add(fmt.Sprintf("u%d-%s", si, field), "system", "unavailable", "Additional context is stored outside this document and is unavailable for selection.", "", true)
			}
		}
		in, out := array(input["messages"]), array(output["messages"])
		fingerprints := make([]string, 0, len(in)+len(out))
		for _, msg := range in {
			fingerprints = append(fingerprints, canonical(msg))
		}
		prev := history[scope]
		skip := 0
		if !delta && len(prev) > 0 && len(in) >= len(prev) {
			replay := true
			for i, msg := range prev {
				if msg != fingerprints[i] {
					replay = false
					break
				}
			}
			if replay {
				skip = len(prev)
			}
		}
		for _, side := range []struct {
			name     string
			messages []any
			skip     int
		}{{"in", in, skip}, {"out", out, 0}} {
			for mi := side.skip; mi < len(side.messages); mi++ {
				msg := object(side.messages[mi])
				r := role(msg["role"])
				before := len(cat.Units)
				parts(array(msg["content"]), fmt.Sprintf("u%d-%s-%d", si, side.name, mi), r, "text", "")
				if r == "user" {
					prompts[scope] = nil
					promptGaps[scope] = false
					for _, u := range cat.Units[before:] {
						if !u.Unavailable {
							prompts[scope] = append(prompts[scope], u.ID)
						} else {
							promptGaps[scope] = true
						}
					}
				}
			}
		}
		if len(in)+len(out) > 0 {
			for _, msg := range out {
				fingerprints = append(fingerprints, canonical(msg))
			}
			history[scope] = fingerprints
		}
		if s["type"] == "tool_call" {
			call := str(input["tool_call_id"])
			if input["arguments"] != nil {
				name := str(input["name"])
				if name == "" {
					name = str(s["name"])
				}
				add(fmt.Sprintf("u%d-call", si), "assistant", "tool_call", display(map[string]any{"name": name, "arguments": input["arguments"]}), call, false)
			}
			if output["result"] != nil {
				if ps, ok := output["result"].([]any); ok && len(ps) > 0 && object(ps[0])["type"] != nil {
					resultParts(ps, fmt.Sprintf("u%d-result", si), call)
				} else {
					resultParts([]any{map[string]any{"type": "text", "text": display(output["result"])}}, fmt.Sprintf("u%d-result", si), call)
				}
			}
		}
		if s["status"] == "error" {
			if text := str(object(s["error"])["message"]); text != "" {
				add(fmt.Sprintf("u%d-error", si), "system", "error", text, "", false)
			}
		}
	}
	return cat
}
