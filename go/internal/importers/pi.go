// Port of cli/import-pi.mjs — piSessionToRun: pi (earendil-works) JSONL
// transcripts → session/v0. Entries are processed in file order (parentId
// ignored by design); user + toolResult messages accumulate as the pending
// input delta, each assistant message becomes an llm_call, and its toolCall
// blocks become tool_call spans closed by the matching toolResult.
package importers

import (
	"encoding/json"
	"fmt"
	"strings"
	"unicode/utf16"
)

func init() { Registry["pi"] = importPi }

/* --------------------------------------------------- JS-undefined modeling
   JSON.stringify drops object keys whose value is undefined but keeps null.
   Decoded Go maps preserve that distinction (absent key vs nil value), so
   piGet returns a sentinel for absent keys and piSetKey drops it again. */

type piUndefinedType struct{}

var piUndefined = piUndefinedType{}

func piIsUndefined(v any) bool {
	_, ok := v.(piUndefinedType)
	return ok
}

// piGet mirrors JS property access: absent key → undefined (sentinel).
func piGet(mm map[string]any, key string) any {
	if mm == nil {
		return piUndefined
	}
	v, ok := mm[key]
	if !ok {
		return piUndefined
	}
	return v
}

// piSetKey mirrors `key: expr` in a JS object literal that later hits
// JSON.stringify: undefined is dropped, everything else (incl. null) kept.
func piSetKey(mm map[string]any, key string, v any) {
	if piIsUndefined(v) {
		return
	}
	mm[key] = v
}

// piNullish mirrors `x == null` / the `??` guard: null or undefined.
func piNullish(v any) bool {
	return v == nil || piIsUndefined(v)
}

// piFalsy approximates JS truthiness (objects/arrays are always truthy).
func piFalsy(v any) bool {
	if piIsUndefined(v) {
		return true
	}
	switch x := v.(type) {
	case nil:
		return true
	case bool:
		return !x
	case string:
		return x == ""
	case float64:
		return x == 0
	}
	return false
}

// piJSString mirrors String(x) for the model/provider coercion sites.
func piJSString(v any) string {
	switch x := v.(type) {
	case string:
		return x
	case bool:
		if x {
			return "true"
		}
		return "false"
	case float64:
		b, _ := json.Marshal(x)
		return string(b)
	case nil:
		return "null"
	}
	return fmt.Sprintf("%v", v) // objects: JS gives [object Object]; unreachable for real payloads
}

// piModelStr mirrors String(x ?? "unknown").
func piModelStr(v any) string {
	if piNullish(v) {
		return "unknown"
	}
	return piJSString(v)
}

func piDataPart(data any) map[string]any {
	return map[string]any{"type": "data", "data": data}
}

/* ------------------------------------------------------------ the mapping */

func piPart(block any) any {
	b := m(block)
	t, hasType := str(b["type"])
	if b == nil || !hasType {
		return piDataPart(block)
	}
	switch t {
	case "text":
		part := map[string]any{"type": "text"}
		if v, ok := b["text"]; ok && v != nil { // block.text ?? ""
			part["text"] = v
		} else {
			part["text"] = ""
		}
		return part
	case "thinking":
		part := map[string]any{"type": "thinking"}
		if v, ok := b["thinking"]; ok && v != nil { // block.thinking ?? ""
			part["text"] = v
		} else {
			part["text"] = ""
		}
		return part
	case "toolCall":
		id, okID := str(b["id"])
		name, okName := str(b["name"])
		if !okID || !okName {
			return piDataPart(block)
		}
		part := map[string]any{"type": "tool_call", "id": id, "name": name}
		piSetKey(part, "arguments", piGet(b, "arguments"))
		return part
	default:
		return block // open-world pass-through
	}
}

func piContent(content any) []any {
	if piNullish(content) { // content == null covers null and undefined
		return []any{}
	}
	if s, ok := str(content); ok {
		return []any{map[string]any{"type": "text", "text": s}}
	}
	items, ok := content.([]any)
	if !ok {
		return []any{piDataPart(content)}
	}
	out := make([]any, 0, len(items))
	for _, b := range items {
		out = append(out, piPart(b))
	}
	return out
}

func piUsage(usage any) map[string]any {
	u := m(usage)
	if u == nil { // !u, plus truthy non-objects whose props are undefined
		return nil
	}
	out := map[string]any{}
	if v, ok := u["input"]; ok && v != nil {
		out["input_tokens"] = v
	}
	if v, ok := u["output"]; ok && v != nil {
		out["output_tokens"] = v
	}
	if v, ok := u["cacheRead"]; ok && v != nil {
		out["cache_read_tokens"] = v
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

// piToolResultValue: a tool result is usually a single text part; surface
// that plainly, else keep the parts (verbatim, incl. the undefined sentinel).
func piToolResultValue(content any) any {
	if items, ok := content.([]any); ok && len(items) == 1 {
		if pm := m(items[0]); strOr(pm["type"], "") == "text" {
			return piGet(pm, "text")
		}
	}
	return content
}

func piFirstText(content any) (string, bool) {
	for _, p := range piContent(content) {
		pm := m(p)
		if strOr(pm["type"], "") != "text" {
			continue
		}
		t, _ := str(pm["text"])
		if trimmed := strings.TrimSpace(t); trimmed != "" {
			return trimmed, true
		}
	}
	return "", false
}

func importPi(in Input) (map[string]any, error) {
	var sessionMeta map[string]any
	var entries []map[string]any
	for _, line := range in.Lines {
		var e any
		if err := json.Unmarshal([]byte(line), &e); err != nil {
			continue
		}
		em := m(e)
		t := strOr(em["type"], "")
		if t == "session" {
			sessionMeta = em
		}
		if t == "message" && !piFalsy(em["message"]) {
			entries = append(entries, em)
		}
	}
	if len(entries) == 0 {
		return nil, nil
	}

	first := entries[0]
	last := entries[len(entries)-1]

	// Name from the opening user turn (pi has no summary line); trim to a title.
	name := in.Fallback
	for _, e := range entries {
		msg := m(e["message"])
		if strOr(msg["role"], "") != "user" {
			continue
		}
		if openerText, ok := piFirstText(msg["content"]); ok {
			u := utf16.Encode([]rune(openerText)) // JS .length/.slice are UTF-16 units
			if len(u) > 80 {
				name = string(utf16.Decode(u[:77])) + "…"
			} else {
				name = openerText
			}
		}
		break // JS finds only the first user entry, textful or not
	}

	rootStart := piGet(sessionMeta, "timestamp")
	if piNullish(rootStart) {
		rootStart = piGet(first, "timestamp")
	}
	root := map[string]any{
		"id":        "root",
		"parent_id": nil,
		"type":      "agent",
		"name":      name,
		"status":    "ok",
	}
	piSetKey(root, "started_at", rootStart)
	piSetKey(root, "ended_at", piGet(last, "timestamp"))
	spans := []any{root}

	pending := []any{}
	var pendingStart any // JS null
	prevTs := piGet(first, "timestamp")
	openTools := map[string]map[string]any{} // toolCallId -> span
	seq := 0

	for _, e := range entries {
		msg := m(e["message"])
		switch strOr(msg["role"], "") {
		case "user":
			if piNullish(pendingStart) {
				pendingStart = piGet(e, "timestamp")
			}
			pending = append(pending, map[string]any{"role": "user", "content": piContent(msg["content"])})
		case "toolResult":
			isError := msg["isError"] == true || msg["is_error"] == true
			var toolSpan map[string]any
			if id, ok := str(msg["toolCallId"]); ok {
				toolSpan = openTools[id]
			}
			if toolSpan != nil {
				piSetKey(toolSpan, "ended_at", piGet(e, "timestamp"))
				out := map[string]any{}
				piSetKey(out, "result", piToolResultValue(msg["content"]))
				if isError {
					out["is_error"] = true
				}
				toolSpan["output"] = out
				if isError {
					toolSpan["status"] = "error"
				}
				delete(openTools, strOr(msg["toolCallId"], ""))
			}
			// The model sees the result on the next call — keep it in the delta.
			if piNullish(pendingStart) {
				pendingStart = piGet(e, "timestamp")
			}
			trPart := map[string]any{"type": "tool_result"}
			piSetKey(trPart, "tool_call_id", piGet(msg, "toolCallId"))
			if isError {
				trPart["is_error"] = true
			}
			trPart["content"] = piContent(msg["content"])
			pending = append(pending, map[string]any{"role": "tool", "content": []any{trPart}})
		case "assistant":
			seq++
			span := map[string]any{
				"id":        fmt.Sprintf("s%d", seq),
				"parent_id": "root",
				"type":      "llm_call",
				"name":      "turn",
				"status":    "ok",
				"model": map[string]any{
					"id":       piModelStr(piGet(msg, "model")),
					"provider": piModelStr(piGet(msg, "provider")),
				},
				"input": map[string]any{"messages": pending},
				"output": map[string]any{"messages": []any{
					map[string]any{"role": "assistant", "content": piContent(msg["content"])},
				}},
			}
			piSetKey(span, "started_at", prevTs)
			piSetKey(span, "ended_at", piGet(e, "timestamp"))
			if usage := piUsage(msg["usage"]); usage != nil {
				span["usage"] = usage
			}
			spans = append(spans, span)
			pending = []any{}
			pendingStart = nil
			blocks, _ := msg["content"].([]any)
			for _, bv := range blocks {
				b := m(bv)
				if strOr(b["type"], "") != "toolCall" {
					continue
				}
				id, ok := str(b["id"])
				if !ok {
					continue
				}
				seq++
				toolSpan := map[string]any{
					"id":        fmt.Sprintf("s%d", seq),
					"parent_id": span["id"],
					"type":      "tool_call",
					"status":    "ok",
				}
				piSetKey(toolSpan, "name", piGet(b, "name"))
				piSetKey(toolSpan, "started_at", piGet(e, "timestamp"))
				input := map[string]any{"tool_call_id": id}
				piSetKey(input, "name", piGet(b, "name"))
				piSetKey(input, "arguments", piGet(b, "arguments"))
				toolSpan["input"] = input
				spans = append(spans, toolSpan)
				openTools[id] = toolSpan
			}
		}
		prevTs = piGet(e, "timestamp")
	}

	// Raw is sacred: a transcript ending on a user/tool turn keeps those
	// messages as a custom span rather than fabricating a reply-less llm_call.
	if len(pending) > 0 {
		seq++
		trailing := map[string]any{
			"id":        fmt.Sprintf("s%d", seq),
			"parent_id": "root",
			"type":      "custom",
			"name":      "trailing message(s) — transcript ends before a reply",
			"input":     map[string]any{"messages": pending},
		}
		start := pendingStart
		if piNullish(start) {
			start = prevTs
		}
		piSetKey(trailing, "started_at", start)
		piSetKey(trailing, "ended_at", prevTs)
		spans = append(spans, trailing)
	}

	metadata := map[string]any{}
	if sessionMeta != nil {
		if !piFalsy(sessionMeta["id"]) {
			metadata["session_id"] = sessionMeta["id"]
		}
		if !piFalsy(sessionMeta["cwd"]) {
			metadata["cwd"] = sessionMeta["cwd"]
		}
	}
	createdAt := piGet(sessionMeta, "timestamp")
	if piNullish(createdAt) {
		createdAt = piGet(first, "timestamp")
	}
	run := map[string]any{
		"schema": "session/v0",
		"name":   name,
		"source": map[string]any{
			"kind": "import", "harness": "pi",
			"label": "pi-import@0.1.0", "fidelity": "reconstructed",
		},
		"metadata": metadata,
		"spans":    spans,
	}
	piSetKey(run, "created_at", createdAt)
	return run, nil
}
