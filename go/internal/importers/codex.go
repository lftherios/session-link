// Port of cli/import-codex.mjs — Codex (openai/codex) rollout JSONL →
// session/v0. Each rollout line is { timestamp, type, payload }: a
// session_meta header (cwd, model_provider, base_instructions), turn_context
// lines (per-turn model), token_count events (usage), and response_item
// lines carrying Responses API items grouped by turn_id. Each turn collapses
// to one llm_call span; its function_calls become tool_call children.
package importers

import (
	"encoding/json"
	"strconv"
	"strings"
	"unicode/utf16"
)

func init() {
	Registry["codex"] = func(in Input) (map[string]any, error) {
		return codexRolloutToRun(in.Lines, in.Fallback), nil
	}
}

func codexDataPart(data any) map[string]any {
	return map[string]any{"type": "data", "data": data}
}

// codexTruthy mirrors JS truthiness for decoded JSON values.
func codexTruthy(v any) bool {
	switch x := v.(type) {
	case nil:
		return false
	case bool:
		return x
	case string:
		return x != ""
	case float64:
		return x != 0
	}
	return true // objects and arrays, even empty
}

// codexString mirrors JS String(x) for decoded JSON values (null included).
func codexString(v any) string {
	switch x := v.(type) {
	case nil:
		return "null"
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
	case []any:
		parts := make([]string, len(x))
		for i, e := range x {
			if e == nil {
				parts[i] = ""
			} else {
				parts[i] = codexString(e)
			}
		}
		return strings.Join(parts, ",")
	default:
		return "[object Object]"
	}
}

// codexMapContent mirrors mapContent.
func codexMapContent(content any) []any {
	if s, ok := str(content); ok {
		if s != "" {
			return []any{map[string]any{"type": "text", "text": s}}
		}
		return []any{}
	}
	items, ok := content.([]any)
	if !ok {
		return []any{}
	}
	out := make([]any, 0, len(items))
	for _, c := range items {
		cm := m(c)
		t, hasType := str(cm["type"])
		switch {
		case cm == nil || !hasType:
			out = append(out, codexDataPart(c))
		case t == "input_text" || t == "output_text" || t == "text":
			var text any = ""
			if v := cm["text"]; v != nil { // c.text ?? ""
				text = v
			}
			out = append(out, map[string]any{"type": "text", "text": text})
		case t == "input_image":
			part := map[string]any{"type": "image"}
			// url: c.image_url ?? c.url — an undefined result is dropped by
			// JSON.stringify, so omit the key when both are absent.
			if v := cm["image_url"]; v != nil {
				part["url"] = v
			} else if v, present := cm["url"]; present {
				part["url"] = v
			}
			out = append(out, part)
		default:
			out = append(out, c) // open-world pass-through
		}
	}
	return out
}

// codexReasoningText mirrors reasoningText.
func codexReasoningText(p map[string]any) string {
	var parts []any
	if a, ok := p["summary"].([]any); ok {
		parts = a
	} else if a, ok := p["content"].([]any); ok {
		parts = a
	}
	pieces := make([]string, 0, len(parts))
	for _, s := range parts {
		if sv, ok := str(s); ok {
			pieces = append(pieces, sv)
			continue
		}
		if v := m(s)["text"]; v != nil { // s?.text ?? ""
			pieces = append(pieces, codexString(v))
		} else {
			pieces = append(pieces, "")
		}
	}
	text := strings.TrimSpace(strings.Join(pieces, "\n"))
	if text != "" {
		return text
	}
	if codexTruthy(p["encrypted_content"]) {
		return "[reasoning]"
	}
	return ""
}

// codexUsageMap mirrors codexUsage.
func codexUsageMap(u any) map[string]any {
	if !codexTruthy(u) {
		return nil
	}
	um := m(u)
	out := map[string]any{}
	for _, kv := range [][2]string{
		{"input_tokens", "input_tokens"},
		{"output_tokens", "output_tokens"},
		{"cached_input_tokens", "cache_read_tokens"},
		{"reasoning_output_tokens", "reasoning_tokens"},
	} {
		if v, ok := um[kv[0]]; ok && v != nil { // u.x != null
			out[kv[1]] = v
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

// codexSystemPrompt mirrors systemPrompt: string | {text: string} | undefined.
func codexSystemPrompt(b any) any {
	if s, ok := str(b); ok {
		return s
	}
	if s, ok := str(m(b)["text"]); ok {
		return s
	}
	return nil
}

// codexFirstText mirrors firstText: first mapped text part whose trimmed
// text is truthy.
func codexFirstText(content any) (string, bool) {
	for _, c := range codexMapContent(content) {
		cm := m(c)
		if strOr(cm["type"], "") != "text" {
			continue
		}
		if s, ok := str(cm["text"]); ok {
			if t := strings.TrimSpace(s); t != "" {
				return t, true
			}
		}
	}
	return "", false
}

// codexTruncate mirrors the JS 80/77 name truncation, which counts and
// slices UTF-16 code units.
func codexTruncate(s string) string {
	u := utf16.Encode([]rune(s))
	if len(u) <= 80 {
		return s
	}
	return string(utf16.Decode(u[:77])) + "…"
}

// codexSameKey mirrors Map key identity for JSON scalars; objects and
// arrays are distinct references per parsed line, so they never match.
func codexSameKey(a, b any) bool {
	switch a.(type) {
	case string, float64, bool:
	default:
		return false
	}
	switch b.(type) {
	case string, float64, bool:
	default:
		return false
	}
	return a == b
}

// codexCallID mirrors String(p.call_id ?? p.id).
func codexCallID(p map[string]any) string {
	if v := p["call_id"]; v != nil {
		return codexString(v)
	}
	if v, ok := p["id"]; ok {
		return codexString(v) // present null → "null"
	}
	return "undefined"
}

// codexStringField mirrors String(p.k) with absent → "undefined".
func codexStringField(p map[string]any, k string) string {
	v, ok := p[k]
	if !ok {
		return "undefined"
	}
	return codexString(v)
}

// codexRolloutToRun ports codexRolloutToRun(lines, fallbackName).
func codexRolloutToRun(lines []string, fallbackName string) map[string]any {
	var metaVal any
	type tmEntry struct{ key, val any }
	var turnModels []tmEntry // insertion-ordered, like a JS Map
	tmSet := func(k, v any) {
		for i := range turnModels {
			if codexSameKey(turnModels[i].key, k) {
				turnModels[i].val = v
				return
			}
		}
		turnModels = append(turnModels, tmEntry{k, v})
	}
	tmGet := func(k any) any {
		for i := range turnModels {
			if codexSameKey(turnModels[i].key, k) {
				return turnModels[i].val
			}
		}
		return nil
	}

	items := []map[string]any{}
	for _, line := range lines {
		var ev any
		if json.Unmarshal([]byte(line), &ev) != nil {
			continue
		}
		e := m(ev)
		t := strOr(e["type"], "")
		switch {
		case t == "session_meta" && codexTruthy(e["payload"]):
			metaVal = e["payload"]
		case t == "turn_context" && codexTruthy(m(e["payload"])["turn_id"]):
			p := m(e["payload"])
			tmSet(p["turn_id"], p["model"])
		case t == "response_item" || t == "event_msg":
			items = append(items, e)
		}
	}
	responseItems := []map[string]any{}
	for _, it := range items {
		if strOr(it["type"], "") == "response_item" {
			responseItems = append(responseItems, it)
		}
	}
	if metaVal == nil && len(responseItems) == 0 {
		return nil
	}

	metaMap := m(metaVal)
	provider := "openai"
	if v := metaMap["model_provider"]; v != nil {
		provider = codexString(v)
	}
	system := codexSystemPrompt(metaMap["base_instructions"])
	var defaultModel any
	for _, e := range turnModels { // [...values()].filter(Boolean).pop()
		if codexTruthy(e.val) {
			defaultModel = e.val
		}
	}
	var createdAt any = "1970-01-01T00:00:00.000Z"
	if v := metaMap["timestamp"]; v != nil {
		createdAt = v
	} else if len(items) > 0 && items[0]["timestamp"] != nil {
		createdAt = items[0]["timestamp"]
	}

	// Name from the first *real* user prompt: a user_message event carries
	// the typed prompt verbatim; injected role:"user" response_items don't.
	var openerText any
	for _, it := range items {
		if strOr(it["type"], "") == "event_msg" && strOr(m(it["payload"])["type"], "") == "user_message" {
			openerText = m(it["payload"])["message"]
			break
		}
	}
	if !codexTruthy(openerText) { // || fallback to the first user response_item
		openerText = nil
		for _, it := range responseItems {
			p := m(it["payload"])
			if strOr(p["type"], "") == "message" && strOr(p["role"], "") == "user" {
				if t, ok := codexFirstText(p["content"]); ok {
					openerText = t
				}
				break
			}
		}
	}
	var name any = fallbackName
	if codexTruthy(openerText) {
		name = openerText
		if s, ok := str(openerText); ok {
			name = codexTruncate(s)
		}
	}

	root := map[string]any{"id": "root", "parent_id": nil, "type": "agent", "name": name, "started_at": createdAt}
	spans := []any{root}

	type codexTool struct {
		callID             string
		name, args         any
		startedTs, endedTs any
		output             any
	}
	pending := []any{}
	reasoningBuf := []string{}
	toolBuf := []*codexTool{}
	var lastUsage any
	turnStartTs := createdAt
	lastTs := createdAt
	seq := 0

	flush := func(assistantContent []any, model any, endedTs any) {
		seq++
		out := []any{}
		for _, r := range reasoningBuf {
			if r != "" {
				out = append(out, map[string]any{"type": "thinking", "text": r})
			}
		}
		out = append(out, assistantContent...)
		for _, tb := range toolBuf {
			out = append(out, map[string]any{"type": "tool_call", "id": tb.callID, "name": tb.name, "arguments": tb.args})
		}
		modelID := "unknown" // String(model ?? defaultModel ?? "unknown")
		if model != nil {
			modelID = codexString(model)
		} else if defaultModel != nil {
			modelID = codexString(defaultModel)
		}
		input := map[string]any{"messages": pending}
		if codexTruthy(system) {
			input["system"] = system
		}
		span := map[string]any{
			"id":         "s" + strconv.Itoa(seq),
			"parent_id":  "root",
			"type":       "llm_call",
			"name":       "turn",
			"started_at": turnStartTs,
			"ended_at":   endedTs,
			"status":     "ok",
			"model":      map[string]any{"id": modelID, "provider": provider},
			"input":      input,
			"output":     map[string]any{"messages": []any{map[string]any{"role": "assistant", "content": out}}},
		}
		if usage := codexUsageMap(lastUsage); usage != nil {
			span["usage"] = usage
		}
		spans = append(spans, span)
		for _, tb := range toolBuf {
			seq++
			started := tb.startedTs
			if started == nil {
				started = endedTs
			}
			ended := tb.endedTs
			if ended == nil {
				ended = endedTs
			}
			spans = append(spans, map[string]any{
				"id":         "s" + strconv.Itoa(seq),
				"parent_id":  span["id"],
				"type":       "tool_call",
				"name":       tb.name,
				"started_at": started,
				"ended_at":   ended,
				"status":     "ok",
				"input":      map[string]any{"name": tb.name, "arguments": tb.args, "tool_call_id": tb.callID},
				"output":     map[string]any{"result": tb.output}, // tb.output ?? null
			})
		}
		pending = []any{}
		reasoningBuf = []string{}
		toolBuf = []*codexTool{}
		lastUsage = nil
		turnStartTs = endedTs
	}

	for _, item := range items {
		ts := lastTs
		if v := item["timestamp"]; v != nil { // item.timestamp ?? lastTs
			ts = v
		}
		p := m(item["payload"])
		if strOr(item["type"], "") == "event_msg" {
			if strOr(p["type"], "") == "token_count" {
				info := m(p["info"])
				if v := info["last_token_usage"]; v != nil {
					lastUsage = v
				} else if v := info["total_token_usage"]; v != nil {
					lastUsage = v
				}
			}
			continue // event_msg never advances lastTs
		}
		switch strOr(p["type"], "") {
		case "message":
			if strOr(p["role"], "") == "assistant" {
				model := tmGet(m(p["internal_chat_message_metadata_passthrough"])["turn_id"])
				flush(codexMapContent(p["content"]), model, ts)
			} else {
				if len(pending) == 0 && len(toolBuf) == 0 && len(reasoningBuf) == 0 {
					turnStartTs = ts
				}
				role := any("user")
				if v := p["role"]; v != nil { // p.role ?? "user"
					role = v
				}
				pending = append(pending, map[string]any{"role": role, "content": codexMapContent(p["content"])})
			}
		case "function_call":
			args := p["arguments"]
			if s, ok := str(args); ok {
				var parsed any
				if json.Unmarshal([]byte(s), &parsed) == nil {
					args = parsed
				} // else keep raw
			}
			nameV := any("tool")
			if codexTruthy(p["name"]) { // p.name || "tool"
				nameV = p["name"]
			}
			toolBuf = append(toolBuf, &codexTool{callID: codexCallID(p), name: nameV, args: args, startedTs: ts})
		case "function_call_output":
			want := codexStringField(p, "call_id")
			for _, tb := range toolBuf {
				if tb.callID == want {
					tb.output = p["output"]
					tb.endedTs = ts
					break
				}
			}
		case "reasoning":
			reasoningBuf = append(reasoningBuf, codexReasoningText(p))
		}
		lastTs = ts
	}

	// An assistant-side buffer with no closing message still becomes a turn;
	// otherwise leftover user input survives as a custom span.
	if len(toolBuf) > 0 || len(reasoningBuf) > 0 {
		flush([]any{}, defaultModel, lastTs)
	} else if len(pending) > 0 {
		seq++
		spans = append(spans, map[string]any{
			"id":         "s" + strconv.Itoa(seq),
			"parent_id":  "root",
			"type":       "custom",
			"name":       "trailing message(s) — no assistant reply",
			"started_at": turnStartTs,
			"ended_at":   lastTs,
			"input":      map[string]any{"messages": pending},
		})
	}

	root["ended_at"] = lastTs
	root["status"] = "ok"

	metadata := map[string]any{}
	if codexTruthy(metaMap["session_id"]) {
		metadata["session_id"] = metaMap["session_id"]
	}
	if codexTruthy(metaMap["cwd"]) {
		metadata["cwd"] = metaMap["cwd"]
	}
	if codexTruthy(metaMap["cli_version"]) {
		metadata["harness_version"] = metaMap["cli_version"]
	}

	return map[string]any{
		"schema":     "session/v0",
		"name":       name,
		"created_at": createdAt,
		"source":     map[string]any{"kind": "import", "harness": "codex", "label": "session-import@0.1.0", "fidelity": "reconstructed"},
		"metadata":   metadata,
		"spans":      spans,
	}
}
