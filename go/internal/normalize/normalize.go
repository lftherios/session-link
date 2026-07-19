// Package normalize ports cli/normalize.mjs: provider wire formats →
// session/v0 llm_call spans. Parity with the JS reference is enforced by
// the testdata/proxy goldens (parsed equality). The implementation works
// on decoded any-trees (map[string]any / []any) so JS's open-world
// pass-through semantics carry over verbatim: unknown block types flow
// through untouched, and anything that would produce a schema-invalid
// part degrades to a {type:"data"} part instead.
package normalize

import (
	"encoding/json"
	"fmt"
	"sort"
	"strings"
)

var paramKeys = []string{
	"temperature", "max_tokens", "max_completion_tokens", "max_output_tokens",
	"top_p", "top_k", "stop", "stop_sequences", "reasoning_effort", "seed",
}

/* ---------------------------------------------------------- any helpers */

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

// tryJSON mirrors JS tryJson: parse strings, pass everything else through.
func tryJSON(v any) any {
	s, ok := v.(string)
	if !ok {
		return v
	}
	var out any
	if err := json.Unmarshal([]byte(s), &out); err != nil {
		return s
	}
	return out
}

func dataPart(data any) map[string]any {
	return map[string]any{"type": "data", "data": data}
}

/* ------------------------------------------------ anthropic /v1/messages */

func anthropicPart(block any) any {
	b := m(block)
	t, hasType := str(b["type"])
	if b == nil || !hasType {
		return dataPart(block)
	}
	switch t {
	case "text":
		return map[string]any{"type": "text", "text": strOr(b["text"], "")}
	case "thinking":
		return map[string]any{"type": "thinking", "text": strOr(b["thinking"], "")}
	case "tool_use":
		id, okID := str(b["id"])
		name, okName := str(b["name"])
		if !okID || !okName {
			return dataPart(block)
		}
		return map[string]any{"type": "tool_call", "id": id, "name": name, "arguments": b["input"]}
	case "tool_result":
		tuID, ok := str(b["tool_use_id"])
		if !ok {
			return dataPart(block)
		}
		out := map[string]any{
			"type":         "tool_result",
			"tool_call_id": tuID,
			"content":      AnthropicContent(b["content"]),
		}
		if v, present := b["is_error"]; present {
			out["is_error"] = v
		}
		return out
	case "image":
		src := m(b["source"])
		if strOr(src["type"], "") == "url" {
			return map[string]any{"type": "image", "url": src["url"], "mime": src["media_type"]}
		}
		return dataPart(map[string]any{
			"kind": "image", "media_type": src["media_type"],
			"note": "inline bytes omitted; see raw",
		})
	default:
		return block // open-world pass-through
	}
}

// AnthropicContent mirrors anthropicContent.
func AnthropicContent(content any) []any {
	if content == nil {
		return []any{}
	}
	if s, ok := str(content); ok {
		return []any{map[string]any{"type": "text", "text": s}}
	}
	items := arr(content)
	out := make([]any, 0, len(items))
	for _, b := range items {
		out = append(out, anthropicPart(b))
	}
	return out
}

func anthropicSystem(system any) (string, bool) {
	if system == nil {
		return "", false
	}
	if s, ok := str(system); ok {
		return s, true
	}
	blocks := arr(system)
	parts := make([]string, 0, len(blocks))
	for _, b := range blocks {
		parts = append(parts, strOr(m(b)["text"], ""))
	}
	return strings.Join(parts, "\n\n"), true
}

// AnthropicUsage mirrors anthropicUsage.
func AnthropicUsage(usage any) map[string]any {
	u := m(usage)
	if u == nil {
		return nil
	}
	out := map[string]any{}
	if v, ok := u["input_tokens"]; ok && v != nil {
		out["input_tokens"] = v
	}
	if v, ok := u["output_tokens"]; ok && v != nil {
		out["output_tokens"] = v
	}
	if v, ok := u["cache_read_input_tokens"]; ok && v != nil {
		out["cache_read_tokens"] = v
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

/* ------------------------------------------- openai /v1/chat/completions */

func openaiChatParts(msg map[string]any) []any {
	parts := []any{}
	if s, ok := str(msg["content"]); ok && s != "" {
		parts = append(parts, map[string]any{"type": "text", "text": s})
	} else if items, ok := msg["content"].([]any); ok {
		for _, p := range items {
			pm := m(p)
			t, hasType := str(pm["type"])
			switch {
			case pm == nil || !hasType:
				parts = append(parts, dataPart(p))
			case t == "text":
				parts = append(parts, map[string]any{"type": "text", "text": strOr(pm["text"], "")})
			case t == "image_url" && m(pm["image_url"])["url"] != nil:
				parts = append(parts, map[string]any{"type": "image", "url": m(pm["image_url"])["url"]})
			default:
				parts = append(parts, p) // open-world pass-through
			}
		}
	}
	for _, tc := range arr(msg["tool_calls"]) {
		tcm := m(tc)
		id, okID := str(tcm["id"])
		fn := m(tcm["function"])
		name, okName := str(fn["name"])
		if okID && okName {
			parts = append(parts, map[string]any{
				"type": "tool_call", "id": id, "name": name,
				"arguments": tryJSON(fn["arguments"]),
			})
		} else {
			parts = append(parts, dataPart(tc))
		}
	}
	return parts
}

func openaiChatMessages(messages []any) []any {
	out := make([]any, 0, len(messages))
	for _, mv := range messages {
		mm := m(mv)
		if strOr(mm["role"], "") == "tool" {
			if tcID, ok := str(mm["tool_call_id"]); ok {
				var content []any
				if s, isStr := str(mm["content"]); isStr {
					content = []any{map[string]any{"type": "text", "text": s}}
				} else {
					content = openaiChatParts(mm)
				}
				out = append(out, map[string]any{
					"role": "tool",
					"content": []any{map[string]any{
						"type": "tool_result", "tool_call_id": tcID, "content": content,
					}},
				})
				continue
			}
		}
		out = append(out, map[string]any{
			"role":    strOr(mm["role"], "user"),
			"content": openaiChatParts(mm),
		})
	}
	return out
}

func openaiTools(tools any) []any {
	items, ok := tools.([]any)
	if !ok {
		return nil
	}
	out := []any{}
	for _, t := range items {
		f := m(t)
		if inner := m(f["function"]); inner != nil {
			f = inner
		}
		name, okName := str(f["name"])
		if !okName {
			continue
		}
		tool := map[string]any{"name": name}
		if d, ok := str(f["description"]); ok && d != "" {
			tool["description"] = d
		}
		if p := f["parameters"]; p != nil && !isFalsy(p) {
			tool["input_schema"] = p
		}
		out = append(out, tool)
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

// isFalsy approximates JS truthiness for the spread-conditional sites.
func isFalsy(v any) bool {
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

func openaiUsage(usage any) map[string]any {
	u := m(usage)
	if u == nil {
		return nil
	}
	pick := func(keys ...string) any {
		for _, k := range keys {
			if v, ok := u[k]; ok && v != nil {
				return v
			}
		}
		return nil
	}
	out := map[string]any{}
	if v := pick("input_tokens", "prompt_tokens"); v != nil {
		out["input_tokens"] = v
	}
	if v := pick("output_tokens", "completion_tokens"); v != nil {
		out["output_tokens"] = v
	}
	var cached any
	if d := m(u["input_tokens_details"]); d != nil && d["cached_tokens"] != nil {
		cached = d["cached_tokens"]
	} else if d := m(u["prompt_tokens_details"]); d != nil && d["cached_tokens"] != nil {
		cached = d["cached_tokens"]
	}
	if cached != nil {
		out["cache_read_tokens"] = cached
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

/* -------------------------------------------------- openai /v1/responses */

func responsesContent(content any) []any {
	if s, ok := str(content); ok {
		return []any{map[string]any{"type": "text", "text": s}}
	}
	items, ok := content.([]any)
	if !ok {
		return []any{}
	}
	out := make([]any, 0, len(items))
	for _, p := range items {
		pm := m(p)
		t, hasType := str(pm["type"])
		switch {
		case pm == nil || !hasType:
			out = append(out, dataPart(p))
		case t == "input_text" || t == "output_text":
			out = append(out, map[string]any{"type": "text", "text": strOr(pm["text"], "")})
		case t == "input_image" && pm["image_url"] != nil && !isFalsy(pm["image_url"]):
			out = append(out, map[string]any{"type": "image", "url": pm["image_url"]})
		default:
			out = append(out, p)
		}
	}
	return out
}

func responsesInputMessages(input any) []any {
	if s, ok := str(input); ok {
		return []any{map[string]any{
			"role": "user", "content": []any{map[string]any{"type": "text", "text": s}},
		}}
	}
	items, ok := input.([]any)
	if !ok {
		return []any{}
	}
	messages := []any{}
	for _, item := range items {
		im := m(item)
		if im["role"] != nil && !isFalsy(im["role"]) {
			messages = append(messages, map[string]any{
				"role": im["role"], "content": responsesContent(im["content"]),
			})
			continue
		}
		t := strOr(im["type"], "")
		callID, hasCall := str(im["call_id"])
		if t == "function_call" && hasCall {
			messages = append(messages, map[string]any{
				"role": "assistant",
				"content": []any{map[string]any{
					"type": "tool_call", "id": callID,
					"name":      strOrAny(im["name"], "function"),
					"arguments": tryJSON(im["arguments"]),
				}},
			})
		} else if t == "function_call_output" && hasCall {
			messages = append(messages, map[string]any{
				"role": "tool",
				"content": []any{map[string]any{
					"type": "tool_result", "tool_call_id": callID,
					"content": []any{map[string]any{"type": "text", "text": jsString(im["output"])}},
				}},
			})
		} else {
			messages = append(messages, map[string]any{"role": "user", "content": []any{dataPart(item)}})
		}
	}
	return messages
}

// strOrAny: JS `x ?? def` where x may be any JSON value.
func strOrAny(v any, def any) any {
	if v == nil {
		return def
	}
	return v
}

// jsString mirrors String(x ?? "") for the function_call_output path.
func jsString(v any) string {
	switch x := v.(type) {
	case nil:
		return ""
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
	default:
		return fmt.Sprintf("%v", x) // objects: JS would give [object Object]; unreachable for real payloads
	}
}

func responsesOutputMessages(output any) []any {
	items, ok := output.([]any)
	if !ok {
		return []any{}
	}
	messages := []any{}
	for _, item := range items {
		im := m(item)
		t := strOr(im["type"], "")
		callID, hasCall := str(im["call_id"])
		switch {
		case t == "message":
			messages = append(messages, map[string]any{
				"role":    strOrAny(im["role"], "assistant"),
				"content": responsesContent(im["content"]),
			})
		case t == "function_call" && hasCall:
			messages = append(messages, map[string]any{
				"role": "assistant",
				"content": []any{map[string]any{
					"type": "tool_call", "id": callID,
					"name":      strOrAny(im["name"], "function"),
					"arguments": tryJSON(im["arguments"]),
				}},
			})
		case t == "reasoning":
			var parts []string
			for _, s := range arr(im["summary"]) {
				parts = append(parts, strOr(m(s)["text"], ""))
			}
			messages = append(messages, map[string]any{
				"role":    "assistant",
				"content": []any{map[string]any{"type": "thinking", "text": strings.Join(parts, "\n")}},
			})
		default:
			messages = append(messages, map[string]any{"role": "assistant", "content": []any{dataPart(item)}})
		}
	}
	return messages
}

/* ---------------------------------------------------------- SSE handling */

// ParseSseText mirrors parseSseText.
func ParseSseText(text string) []any {
	events := []any{}
	for _, line := range strings.Split(strings.ReplaceAll(text, "\r\n", "\n"), "\n") {
		if !strings.HasPrefix(line, "data:") {
			continue
		}
		payload := strings.TrimSpace(line[5:])
		if payload == "" || payload == "[DONE]" {
			continue
		}
		var ev any
		if err := json.Unmarshal([]byte(payload), &ev); err == nil {
			events = append(events, ev)
		}
	}
	return events
}

// AssembleAnthropicSse mirrors assembleAnthropicSse.
func AssembleAnthropicSse(events []any) map[string]any {
	var message map[string]any
	blocks := map[int]map[string]any{}
	maxIndex := -1
	for _, evv := range events {
		ev := m(evv)
		switch strOr(ev["type"], "") {
		case "message_start":
			message = deepCopyMap(m(ev["message"]))
			if message == nil {
				message = map[string]any{}
			}
		case "content_block_start":
			idx, ok := intOf(ev["index"])
			if !ok {
				continue
			}
			blk := deepCopyMap(m(ev["content_block"]))
			if blk == nil {
				blk = map[string]any{}
			}
			blocks[idx] = blk
			if idx > maxIndex {
				maxIndex = idx
			}
		case "content_block_delta":
			idx, ok := intOf(ev["index"])
			if !ok {
				continue
			}
			b := blocks[idx]
			d := m(ev["delta"])
			if b == nil || d == nil {
				continue
			}
			switch strOr(d["type"], "") {
			case "text_delta":
				b["text"] = strOr(b["text"], "") + strOr(d["text"], "")
			case "thinking_delta":
				b["thinking"] = strOr(b["thinking"], "") + strOr(d["thinking"], "")
			case "input_json_delta":
				b["_json"] = strOr(b["_json"], "") + strOr(d["partial_json"], "")
			case "signature_delta":
				b["signature"] = strOr(b["signature"], "") + strOr(d["signature"], "")
			}
		case "message_delta":
			if message != nil {
				for k, v := range m(ev["delta"]) {
					message[k] = v
				}
				if u := m(ev["usage"]); u != nil {
					merged := map[string]any{}
					for k, v := range m(message["usage"]) {
						merged[k] = v
					}
					for k, v := range u {
						merged[k] = v
					}
					message["usage"] = merged
				}
			}
		}
	}
	if message == nil {
		return nil
	}
	content := []any{}
	indices := make([]int, 0, len(blocks))
	for i := range blocks {
		indices = append(indices, i)
	}
	sort.Ints(indices)
	for _, i := range indices {
		b := blocks[i]
		if b == nil {
			continue
		}
		if rawJSON, present := b["_json"]; present {
			s, _ := str(rawJSON)
			if s == "" {
				b["input"] = map[string]any{}
			} else {
				var parsed any
				if err := json.Unmarshal([]byte(s), &parsed); err == nil {
					b["input"] = parsed
				}
				// parse failure: keep partial json out of input, like JS
			}
			delete(b, "_json")
		}
		content = append(content, b)
	}
	message["content"] = content
	return message
}

// AssembleOpenaiChatSse mirrors assembleOpenaiChatSse.
func AssembleOpenaiChatSse(events []any) map[string]any {
	var base map[string]any
	role := "assistant"
	content := ""
	toolCalls := map[int]map[string]any{}
	maxIdx := -1
	var finish any
	var usage any
	for _, evv := range events {
		ev := m(evv)
		if u := ev["usage"]; u != nil && !isFalsy(u) {
			usage = u
		}
		if base == nil && ev["id"] != nil && !isFalsy(ev["id"]) {
			base = map[string]any{"id": ev["id"], "model": ev["model"], "created": ev["created"]}
		}
		choices := arr(ev["choices"])
		if len(choices) == 0 {
			continue
		}
		choice := m(choices[0])
		d := m(choice["delta"])
		if r, ok := str(d["role"]); ok && r != "" {
			role = r
		}
		if c, ok := str(d["content"]); ok {
			content += c
		}
		for _, tcv := range arr(d["tool_calls"]) {
			tc := m(tcv)
			idx := 0
			if i, ok := intOf(tc["index"]); ok {
				idx = i
			}
			slot := toolCalls[idx]
			if slot == nil {
				slot = map[string]any{
					"id": "", "type": "function",
					"function": map[string]any{"name": "", "arguments": ""},
				}
				toolCalls[idx] = slot
				if idx > maxIdx {
					maxIdx = idx
				}
			}
			if id, ok := str(tc["id"]); ok && id != "" {
				slot["id"] = id
			}
			fn := m(tc["function"])
			sfn := m(slot["function"])
			if n, ok := str(fn["name"]); ok && n != "" {
				sfn["name"] = strOr(sfn["name"], "") + n
			}
			if a, ok := str(fn["arguments"]); ok && a != "" {
				sfn["arguments"] = strOr(sfn["arguments"], "") + a
			}
		}
		if f := choice["finish_reason"]; f != nil && !isFalsy(f) {
			finish = f
		}
	}
	if base == nil && content == "" && len(toolCalls) == 0 {
		return nil
	}
	calls := []any{}
	idxs := make([]int, 0, len(toolCalls))
	for i := range toolCalls {
		idxs = append(idxs, i)
	}
	sort.Ints(idxs)
	for _, i := range idxs {
		calls = append(calls, toolCalls[i])
	}
	message := map[string]any{"role": role}
	if content != "" {
		message["content"] = content
	} else if len(calls) > 0 {
		message["content"] = nil
	} else {
		message["content"] = ""
	}
	if len(calls) > 0 {
		message["tool_calls"] = calls
	}
	out := map[string]any{}
	for k, v := range base {
		out[k] = v
	}
	out["object"] = "chat.completion"
	out["choices"] = []any{map[string]any{
		"index": float64(0), "message": message, "finish_reason": finish,
	}}
	if usage != nil {
		out["usage"] = usage
	}
	return out
}

// AssembleResponsesSse mirrors assembleResponsesSse.
func AssembleResponsesSse(events []any) map[string]any {
	for i := len(events) - 1; i >= 0; i-- {
		ev := m(events[i])
		if strOr(ev["type"], "") == "response.completed" {
			if r := m(ev["response"]); r != nil {
				return r
			}
		}
	}
	return nil
}

/* ----------------------------------------------------------- the builder */

var spanNames = map[string]string{
	"anthropic.messages": "messages.create",
	"openai.chat":        "chat.completions",
	"openai.responses":   "responses.create",
}

// BuildContext mirrors buildLlmSpan's ctx.
type BuildContext struct {
	ID             string
	ParentID       string
	Request        any
	Response       any
	StartedAt      string
	EndedAt        string
	HTTPStatus     int
	Streamed       bool
	CaptureGap     bool
	TransportError string // "" = none
}

// BuildLlmSpan mirrors buildLlmSpan.
func BuildLlmSpan(kind string, ctx BuildContext) map[string]any {
	req := m(ctx.Request)
	if req == nil {
		req = map[string]any{}
	}
	response := m(ctx.Response)
	ok := ctx.TransportError == "" && ctx.HTTPStatus >= 200 && ctx.HTTPStatus < 300

	provider := "openai"
	if kind == "anthropic.messages" {
		provider = "anthropic"
	}
	modelID := "unknown"
	if mv, present := req["model"]; present && mv != nil {
		modelID = jsString(mv)
	}
	var rawRequest any
	if ctx.Request != nil {
		rawRequest = ctx.Request
	}
	var rawResponse any
	if ctx.Response != nil {
		rawResponse = ctx.Response
	}
	span := map[string]any{
		"id":         ctx.ID,
		"parent_id":  ctx.ParentID,
		"type":       "llm_call",
		"name":       spanNames[kind],
		"started_at": ctx.StartedAt,
		"ended_at":   ctx.EndedAt,
		"status":     map[bool]string{true: "ok", false: "error"}[ok],
		"model":      map[string]any{"id": modelID, "provider": provider},
		"input":      map[string]any{"messages": []any{}},
		"output":     map[string]any{"messages": []any{}},
		"raw":        map[string]any{"request": rawRequest, "response": rawResponse},
	}
	params := map[string]any{}
	for _, k := range paramKeys {
		if v, present := req[k]; present {
			params[k] = v
		}
	}
	if len(params) > 0 {
		span["params"] = params
	}
	if ctx.Streamed {
		span["metadata"] = map[string]any{"stream": true}
	}
	if ctx.CaptureGap {
		span["fidelity"] = "partial"
	}
	if !ok {
		msg := ctx.TransportError
		if msg == "" {
			if em, okE := str(m(response["error"])["message"]); okE {
				msg = em
			} else {
				msg = fmt.Sprintf("HTTP %d", ctx.HTTPStatus)
			}
		}
		span["error"] = map[string]any{"message": msg}
	}

	input := span["input"].(map[string]any)
	output := span["output"].(map[string]any)

	switch kind {
	case "anthropic.messages":
		if system, present := anthropicSystem(req["system"]); present && system != "" {
			input["system"] = system
		}
		messages := []any{}
		for _, mv := range arr(req["messages"]) {
			mm := m(mv)
			messages = append(messages, map[string]any{
				"role":    strOrAny(mm["role"], "user"),
				"content": AnthropicContent(mm["content"]),
			})
		}
		input["messages"] = messages
		tools := []any{}
		for _, tv := range arr(req["tools"]) {
			t := m(tv)
			name, okName := str(t["name"])
			if !okName {
				continue
			}
			tool := map[string]any{"name": name}
			if d, ok := str(t["description"]); ok && d != "" {
				tool["description"] = d
			}
			if s := t["input_schema"]; s != nil && !isFalsy(s) {
				tool["input_schema"] = s
			}
			tools = append(tools, tool)
		}
		if len(tools) > 0 {
			input["tools"] = tools
		}
		if _, isArr := response["content"].([]any); ok && isArr {
			output["messages"] = []any{map[string]any{
				"role": "assistant", "content": AnthropicContent(response["content"]),
			}}
		}
		if usage := AnthropicUsage(response["usage"]); usage != nil {
			span["usage"] = usage
		}
	case "openai.chat":
		input["messages"] = openaiChatMessages(arr(req["messages"]))
		if tools := openaiTools(req["tools"]); tools != nil {
			input["tools"] = tools
		}
		choices := arr(response["choices"])
		if ok && len(choices) > 0 {
			if msg := m(m(choices[0])["message"]); msg != nil {
				output["messages"] = openaiChatMessages([]any{msg})
			}
		}
		if usage := openaiUsage(response["usage"]); usage != nil {
			span["usage"] = usage
		}
	case "openai.responses":
		if instr, okI := str(req["instructions"]); okI && instr != "" {
			input["system"] = instr
		}
		input["messages"] = responsesInputMessages(req["input"])
		if tools := openaiTools(req["tools"]); tools != nil {
			input["tools"] = tools
		}
		if ok {
			output["messages"] = responsesOutputMessages(response["output"])
		}
		if usage := openaiUsage(response["usage"]); usage != nil {
			span["usage"] = usage
		}
	}

	return span
}

/* --------------------------------------------------------------- utils */

func intOf(v any) (int, bool) {
	f, ok := v.(float64)
	if !ok {
		return 0, false
	}
	return int(f), true
}

func deepCopyMap(src map[string]any) map[string]any {
	if src == nil {
		return nil
	}
	b, err := json.Marshal(src)
	if err != nil {
		return map[string]any{}
	}
	var out map[string]any
	if json.Unmarshal(b, &out) != nil {
		return map[string]any{}
	}
	return out
}
