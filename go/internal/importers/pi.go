// Port of cli/import-pi.mjs — piSessionToRun: pi (earendil-works) JSONL
// transcripts → session/v0. pi stores a session as a tree, and the import is
// the branch that ends at the file's last entry, the one pi itself resumes;
// turns on branches the person went back from are counted, not replayed.
// User + toolResult messages accumulate as the pending input delta, each
// assistant message becomes an llm_call, and its toolCall blocks become
// tool_call spans closed by the matching toolResult.
package importers

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"unicode/utf16"
)

// omp (oh-my-pi) grew out of pi and keeps its transcript: the same header,
// tree of entries and message shapes. What it adds is noted where it matters.
func init() {
	Registry["pi"] = func(in Input) (map[string]any, error) { return importPi(in, "pi") }
	Registry["omp"] = func(in Input) (map[string]any, error) { return importPi(in, "omp") }
}

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

// piBlobHash is the name omp gives a blob: the SHA-256 of its bytes.
var piBlobHash = regexp.MustCompile(`^[a-f0-9]{64}$`)

// piImages turns the images in mapped content into image parts. pi records an
// image as base64 with its type. omp moves the bytes to its blob store and
// leaves a reference, which resolves while that store is beside the
// transcript. An image that cannot be shown is kept as recorded, as data: an
// image part has to say where its image is.
func piImages(parts []any, blobs string) []any {
	for i, part := range parts {
		b := m(part)
		if strOr(b["type"], "") != "image" {
			continue
		}
		parts[i] = piDataPart(part)
		data := strOr(b["data"], "")
		if hash, reference := strings.CutPrefix(data, "blob:sha256:"); reference {
			data = ""
			if file := filepath.Join(blobs, hash); blobs != "" && piBlobHash.MatchString(hash) {
				if info, err := os.Stat(file); err == nil && info.Size() <= 32<<20 {
					if raw, err := os.ReadFile(file); err == nil {
						data = base64.StdEncoding.EncodeToString(raw)
					}
				}
			}
		}
		if image, ok := imagePart(data, strOr(b["mimeType"], "")); ok {
			parts[i] = image
		}
	}
	return parts
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

// piActiveBranch returns the entries on the path from a session's last entry
// back to its first, oldest first. Going back to an earlier point and carrying
// on from there leaves the turns that were given up in the file, as siblings.
// A session from before pi recorded the tree, or one whose links do not
// resolve, is a single branch in file order.
func piActiveBranch(entries []map[string]any) []map[string]any {
	if len(entries) == 0 {
		return entries
	}
	byID := map[string]map[string]any{}
	for _, e := range entries {
		if id, ok := str(e["id"]); ok {
			byID[id] = e
		}
	}
	var branch []map[string]any
	seen := map[string]bool{}
	for e := entries[len(entries)-1]; e != nil; {
		id, ok := str(e["id"])
		if !ok || seen[id] {
			return entries
		}
		seen[id] = true
		branch = append(branch, e)
		parent, linked := str(e["parentId"])
		if !linked {
			break
		}
		if e = byID[parent]; e == nil {
			return entries
		}
	}
	for i, j := 0, len(branch)-1; i < j; i, j = i+1, j-1 {
		branch[i], branch[j] = branch[j], branch[i]
	}
	return branch
}

func piIsMessage(e map[string]any) bool {
	return strOr(e["type"], "") == "message" && !piFalsy(e["message"])
}

func importPi(in Input, harness string) (map[string]any, error) {
	var sessionMeta map[string]any
	var recorded []map[string]any // every entry after the header, in file order
	sessionName, messages := "", 0
	slotTitle, changedTitle := "", "" // omp: the title line it rewrites in place, and its latest title change
	for _, line := range in.Lines {
		var e any
		if err := json.Unmarshal([]byte(line), &e); err != nil {
			continue
		}
		em := m(e)
		switch strOr(em["type"], "") {
		case "":
			continue
		case "session":
			sessionMeta = em
			continue
		case "title":
			slotTitle = strings.TrimSpace(strOr(em["title"], ""))
			continue
		case "title_change":
			changedTitle = strings.TrimSpace(strOr(em["title"], ""))
		case "session_info":
			// The latest one names the session; an empty name clears it.
			sessionName = strings.TrimSpace(strOr(em["name"], ""))
		}
		if piIsMessage(em) {
			messages++
		}
		recorded = append(recorded, em)
	}
	branch := piActiveBranch(recorded)
	var entries []map[string]any
	for _, e := range branch {
		if piIsMessage(e) {
			entries = append(entries, e)
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
	// A recorded name beats the opening prompt: pi's session name, or omp's
	// current title, its latest title change, then the title in its header.
	for _, recordedName := range []string{sessionName, slotTitle, changedTitle, strings.TrimSpace(strOr(sessionMeta["title"], ""))} {
		if recordedName != "" {
			name = recordedName
			break
		}
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
	agentCalls := map[string]string{} // omp: sub-agent id -> the task call that started it
	var pendingStart any              // JS null
	prevTs := piGet(first, "timestamp")
	openTools := map[string]map[string]any{} // toolCallId -> span
	seq := 0

	for _, e := range branch {
		// A summary pi wrote in place of turns it compacted away, or of a
		// branch the person went back from, is context the model was given.
		if t := strOr(e["type"], ""); t == "compaction" || t == "branch_summary" {
			if summary := strOr(e["summary"], ""); summary != "" {
				if piNullish(pendingStart) {
					pendingStart = piGet(e, "timestamp")
				}
				pending = append(pending, map[string]any{"role": "system", "content": []any{map[string]any{"type": "text", "text": summary}}})
			}
			continue
		}
		// omp records when a tool actually started, after the reply that asked
		// for it; the tool's span starts then.
		if data := m(e["data"]); strOr(e["type"], "") == "custom" && strOr(e["customType"], "") == "tool_execution_start" {
			if span := openTools[strOr(data["toolCallId"], "")]; span != nil && strOr(data["startedAt"], "") != "" {
				span["started_at"] = data["startedAt"]
			}
			continue
		}
		if !piIsMessage(e) {
			continue
		}
		msg := m(e["message"])
		switch role := strOr(msg["role"], ""); role {
		case "user", "developer":
			// A developer message is the harness speaking to the model between
			// turns, as omp does to a sub-agent; it is context, not a prompt.
			if piNullish(pendingStart) {
				pendingStart = piGet(e, "timestamp")
			}
			pending = append(pending, map[string]any{"role": role, "content": piImages(piContent(msg["content"]), in.Blobs)})
		case "toolResult":
			isError := msg["isError"] == true || msg["is_error"] == true
			// omp's task tool reports the sub-agents it started, by id.
			details := m(msg["details"])
			for _, started := range append(arr(details["progress"]), arr(details["results"])...) {
				if id := strOr(m(started)["id"], ""); id != "" {
					agentCalls[id] = strOr(msg["toolCallId"], "")
				}
			}
			var toolSpan map[string]any
			if id, ok := str(msg["toolCallId"]); ok {
				toolSpan = openTools[id]
			}
			if toolSpan != nil {
				piSetKey(toolSpan, "ended_at", piGet(e, "timestamp"))
				out := map[string]any{}
				result := piToolResultValue(msg["content"])
				if items, ok := result.([]any); ok {
					result = piImages(piContent(items), in.Blobs)
				}
				piSetKey(out, "result", result)
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
			trPart["content"] = piImages(piContent(msg["content"]), in.Blobs)
			pending = append(pending, map[string]any{"role": "tool", "content": []any{trPart}})
		case "assistant":
			seq++
			status := "ok"
			if strOr(msg["stopReason"], "") == "error" {
				status = "error"
			}
			span := map[string]any{
				"id":        fmt.Sprintf("s%d", seq),
				"parent_id": "root",
				"type":      "llm_call",
				"name":      "turn",
				"status":    status,
				"model": map[string]any{
					"id":       piModelStr(piGet(msg, "model")),
					"provider": piModelStr(piGet(msg, "provider")),
				},
				"input": map[string]any{"messages": pending},
				"output": map[string]any{"messages": []any{
					map[string]any{"role": "assistant", "content": piImages(piContent(msg["content"]), in.Blobs)},
				}},
			}
			piSetKey(span, "started_at", prevTs)
			piSetKey(span, "ended_at", piGet(e, "timestamp"))
			if usage := piUsage(msg["usage"]); usage != nil {
				span["usage"] = usage
			}
			if failure := strOr(msg["errorMessage"], ""); failure != "" {
				span["error"] = map[string]any{"message": failure}
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
	// Messages on branches the person went back from stay out of the import.
	if skipped := messages - len(entries); skipped != 0 {
		metadata["branch_entries_skipped"] = skipped
	}
	createdAt := piGet(sessionMeta, "timestamp")
	if piNullish(createdAt) {
		createdAt = piGet(first, "timestamp")
	}
	run := map[string]any{
		"schema": "session/v0",
		"name":   name,
		"source": map[string]any{
			"kind": "import", "harness": harness,
			"label": harness + "-import@0.1.0", "fidelity": "reconstructed",
		},
		"metadata": metadata,
		"spans":    spans,
	}
	piSetKey(run, "created_at", createdAt)
	adoptAgents(run, harness, in.Agents, agentCalls)
	return run, nil
}
