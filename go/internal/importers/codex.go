// Port of cli/import-codex.mjs — Codex (openai/codex) rollout JSONL →
// session/v0. Each rollout line is { timestamp, type, payload }: a
// session_meta header (cwd, model_provider, base_instructions), turn_context
// lines (per-turn model), token_count events (usage), and response_item
// lines carrying Responses API items grouped by turn_id. Each turn collapses
// to one llm_call span whose reply keeps reasoning and tool calls in recorded
// order. Function calls, custom tool calls such as apply_patch, local shell
// calls, tool searches and web searches become tool_call children. Any other
// item is kept verbatim in a custom span.
package importers

import (
	"encoding/json"
	"strconv"
	"strings"
	"unicode/utf16"
)

func init() {
	Registry["codex"] = func(in Input) (map[string]any, error) {
		run, agentCalls := codexRolloutToRun(in.Lines, in.Fallback)
		if run != nil {
			adoptAgents(run, "codex", in.Agents, agentCalls)
		}
		return run, nil
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
			if _, located := str(part["url"]); !located {
				part = codexDataPart(c) // an image part has to say where its image is
			}
			out = append(out, part)
		default:
			out = append(out, c) // open-world pass-through
		}
	}
	return out
}

// Prefer a readable summary, falling back to recorded content. Encrypted data
// signals that a reasoning event exists; it is not readable reasoning text.
func codexReasoningText(p map[string]any) string {
	for _, field := range []string{"summary", "content"} {
		parts, _ := p[field].([]any)
		pieces := make([]string, 0, len(parts))
		for _, s := range parts {
			if sv, ok := str(s); ok {
				pieces = append(pieces, sv)
			} else if v := m(s)["text"]; v != nil {
				pieces = append(pieces, codexString(v))
			}
		}
		if text := strings.TrimSpace(strings.Join(pieces, "\n")); text != "" {
			return text
		}
	}
	return ""
}

func codexReasoningPart(p map[string]any) map[string]any {
	part := map[string]any{"type": "thinking", "text": codexReasoningText(p)}
	if part["text"] == "" {
		part["unavailable"], part["reason"] = true, "not_recorded"
		if codexTruthy(p["encrypted_content"]) {
			part["reason"] = "encrypted"
		}
	}
	return part
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

// codexCallID is the id a call's result names: call_id, else the item's own
// id. A call recorded with neither gets fallback, so calls stay distinct.
func codexCallID(p map[string]any, fallback string) string {
	if v := p["call_id"]; v != nil {
		return codexString(v)
	}
	if v := p["id"]; v != nil {
		return codexString(v)
	}
	return fallback
}

// codexRolloutToRun ports codexRolloutToRun(lines, fallbackName). It also
// returns, for each sub-agent the thread spawned, the call that spawned it,
// keyed by the task path Codex gave the agent.
func codexRolloutToRun(lines []string, fallbackName string) (map[string]any, map[string]string) {
	agentCalls := map[string]string{}
	inherited := 0
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
		// A thread that was forked, as a sub-agent's is, opens with its own
		// header and then replays its parent's, header included. It says where
		// its own history starts; what comes before was the parent's.
		if start, forked := m(metaVal)["subagent_history_start_ordinal"].(float64); forked && t != "session_meta" {
			if ordinal, numbered := e["ordinal"].(float64); numbered && ordinal < start {
				if t == "response_item" {
					inherited++
				}
				continue
			}
		}
		switch {
		case t == "session_meta" && codexTruthy(e["payload"]):
			if metaVal == nil {
				metaVal = e["payload"]
			}
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
		return nil, nil
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
				t, ok := codexFirstText(p["content"])
				if ok && strings.HasPrefix(t, "<") {
					continue // a block Codex wrote itself, such as <environment_context>
				}
				if ok {
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
		span               map[string]any // set once the turn is written, so a late result still lands
	}
	pending := []any{}
	activity := []any{} // reasoning and tool calls since the last reply, in recorded order
	toolBuf := []*codexTool{}
	awaiting := map[string]*codexTool{} // call id -> call with no result yet
	calls := 0
	var lastUsage any
	turnStartTs := createdAt
	lastTs := createdAt
	seq := 0

	// keep preserves a rollout item this importer has no mapping for. It stays
	// in the document as recorded instead of disappearing from the session.
	keep := func(p map[string]any, ts any) {
		seq++
		spans = append(spans, map[string]any{
			"id":         "s" + strconv.Itoa(seq),
			"parent_id":  "root",
			"type":       "custom",
			"name":       "codex " + strOr(p["type"], "item"),
			"started_at": ts,
			"ended_at":   ts,
			"raw":        map[string]any{"item": p},
		})
	}

	flush := func(assistantContent []any, model any, endedTs any) {
		seq++
		out := []any{}
		out = append(out, activity...)
		out = append(out, assistantContent...)
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
			tb.span = map[string]any{
				"id":         "s" + strconv.Itoa(seq),
				"parent_id":  span["id"],
				"type":       "tool_call",
				"name":       tb.name,
				"started_at": started,
				"ended_at":   ended,
				"status":     "ok",
				"input":      map[string]any{"name": tb.name, "arguments": tb.args, "tool_call_id": tb.callID},
				"output":     map[string]any{"result": tb.output}, // tb.output ?? null
			}
			spans = append(spans, tb.span)
		}
		pending = []any{}
		activity = []any{}
		toolBuf = []*codexTool{}
		lastUsage = nil
		turnStartTs = endedTs
	}

	// call records one tool call: a part in the reply that made it, and a span
	// its result closes.
	call := func(p map[string]any, name, args, ts any) *codexTool {
		calls++
		tb := &codexTool{callID: codexCallID(p, strOr(p["type"], "call")+"_"+strconv.Itoa(calls)), name: name, args: args, startedTs: ts}
		activity = append(activity, map[string]any{"type": "tool_call", "id": tb.callID, "name": tb.name, "arguments": tb.args})
		toolBuf = append(toolBuf, tb)
		awaiting[tb.callID] = tb
		return tb
	}
	// result closes the call an output item names. One that names no known
	// call is kept as recorded.
	result := func(p map[string]any, output, ts any) {
		tb := awaiting[codexString(p["call_id"])]
		if p["call_id"] == nil || tb == nil {
			keep(p, ts)
			return
		}
		// spawn_agent answers with the task path of the agent it started.
		if text, ok := str(output); ok && tb.name == "spawn_agent" {
			var spawned map[string]any
			if json.Unmarshal([]byte(text), &spawned) == nil && strOr(spawned["task_name"], "") != "" {
				agentCalls[strOr(spawned["task_name"], "")] = tb.callID
			}
		}
		delete(awaiting, tb.callID)
		tb.output, tb.endedTs = output, ts
		if tb.span != nil {
			tb.span["output"], tb.span["ended_at"] = map[string]any{"result": output}, ts
		}
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
				if len(pending) == 0 && len(activity) == 0 {
					turnStartTs = ts
				}
				role := any("user")
				if v := p["role"]; v != nil { // p.role ?? "user"
					role = v
				}
				pending = append(pending, map[string]any{"role": role, "content": codexMapContent(p["content"])})
			}
		case "function_call", "custom_tool_call":
			// A function call carries JSON arguments as a string; a custom tool
			// such as apply_patch carries free-form text, kept as written.
			args := p["input"]
			if strOr(p["type"], "") == "function_call" {
				args = p["arguments"]
				if s, ok := str(args); ok {
					var parsed any
					if json.Unmarshal([]byte(s), &parsed) == nil {
						args = parsed
					} // else keep raw
				}
			}
			nameV := any("tool")
			if codexTruthy(p["name"]) { // p.name || "tool"
				nameV = p["name"]
			}
			call(p, nameV, args, ts)
		case "local_shell_call":
			call(p, "local_shell", p["action"], ts)
		case "tool_search_call":
			call(p, "tool_search", p["arguments"], ts)
		case "web_search_call":
			// The search runs on the provider's side; no result is recorded.
			call(p, "web_search", p["action"], ts)
		case "image_generation_call":
			// The provider draws the image and returns it with the item, along
			// with the prompt it drew from.
			image, ok := imagePart(strOr(p["result"], ""), "")
			if !ok {
				keep(p, ts)
				break
			}
			var args any
			if prompt := strOr(p["revised_prompt"], ""); prompt != "" {
				args = map[string]any{"revised_prompt": prompt}
			}
			tb := call(p, "image_generation", args, ts)
			delete(awaiting, tb.callID)
			tb.output, tb.endedTs = []any{image}, ts
		case "function_call_output", "custom_tool_call_output":
			// A result is text, or a list of content items that can hold images.
			output := p["output"]
			if items, ok := output.([]any); ok {
				output = codexMapContent(items)
			}
			result(p, output, ts)
		case "tool_search_output":
			result(p, p["tools"], ts)
		case "agent_message":
			// Agents in a team write to each other. What is addressed to this
			// thread's own agent is its task, as a prompt is; anything else a
			// thread receives is context. The payload of a task is recorded as
			// encrypted content, kept as it is.
			content := []any{}
			for _, c := range arr(p["content"]) {
				if cm := m(c); strOr(cm["type"], "") == "input_text" {
					content = append(content, map[string]any{"type": "text", "text": strOr(cm["text"], "")})
				} else {
					content = append(content, codexDataPart(c))
				}
			}
			role := "system"
			if path := strOr(metaMap["agent_path"], ""); path != "" && strOr(p["recipient"], "") == path {
				role = "user"
			}
			if len(pending) == 0 && len(activity) == 0 {
				turnStartTs = ts
			}
			pending = append(pending, map[string]any{"role": role, "content": content})
		case "reasoning":
			activity = append(activity, codexReasoningPart(p))
		default:
			if p != nil {
				keep(p, ts)
			}
		}
		lastTs = ts
	}

	// An assistant-side buffer with no closing message still becomes a turn;
	// otherwise leftover user input survives as a custom span.
	if len(activity) > 0 {
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
	if inherited != 0 {
		metadata["inherited_items_skipped"] = inherited
	}

	return map[string]any{
		"schema":     "session/v0",
		"name":       name,
		"created_at": createdAt,
		"source":     map[string]any{"kind": "import", "harness": "codex", "label": "session-import@0.1.0", "fidelity": "reconstructed"},
		"metadata":   metadata,
		"spans":      spans,
	}, agentCalls
}
