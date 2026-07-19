// Port of cli/import-hermes.mjs — buildHermesRun(session, messages).
// Hermes stores an OpenAI-style chat log (role/content/tool_calls/
// tool_call_id/reasoning/token_count); the walk mirrors the OpenAI
// normalizer: user/tool rows accumulate as the input delta, each assistant
// row becomes an llm_call, its tool_calls become tool_call spans closed by
// the matching role:"tool" rows.
package importers

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

func init() { Registry["hermes"] = importHermes }

func importHermes(in Input) (map[string]any, error) {
	session := in.Session
	if session == nil {
		session = map[string]any{}
	}
	return buildHermesRun(session, in.Messages), nil
}

/* ------------------------------------------------------------- helpers */

// hermesTruthy mirrors JS truthiness for the ||/! sites.
func hermesTruthy(v any) bool {
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
	return true // objects/arrays are always truthy
}

// hermesString mirrors String(x) for the coercion sites.
func hermesString(v any) string {
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
	default:
		b, _ := json.Marshal(x)
		return string(b) // unreachable for real payloads
	}
}

// hermesIsoSec mirrors isoSec: REAL seconds → ISO-8601 with milliseconds.
// Non-numbers coerce to 0 (typeof check), ms truncate toward zero like
// JS TimeClip.
func hermesIsoSec(v any) string {
	sec, _ := v.(float64)
	ms := int64(sec * 1000)
	return time.UnixMilli(ms).UTC().Format("2006-01-02T15:04:05.000") + "Z"
}

func hermesDataPart(data any) map[string]any {
	return map[string]any{"type": "data", "data": data}
}

/* ------------------------------------------------------------- content */

func hermesPart(p any) any {
	if s, ok := str(p); ok {
		return map[string]any{"type": "text", "text": s}
	}
	pm := m(p)
	if t, ok := str(pm["type"]); ok {
		if t == "text" {
			txt := pm["text"] // p.text ?? ""
			if txt == nil {
				txt = ""
			}
			return map[string]any{"type": "text", "text": txt}
		}
		return p // open-world pass-through (image_url, etc.)
	}
	if txt, ok := str(pm["text"]); ok {
		return map[string]any{"type": "text", "text": txt}
	}
	return hermesDataPart(p)
}

// hermesContent mirrors hermesContent: content is usually a plain string,
// but can be a JSON list/dict.
func hermesContent(raw any) []any {
	if raw == nil {
		return []any{}
	}
	v := raw
	if s, ok := str(raw); ok {
		t := strings.TrimSpace(s)
		if strings.HasPrefix(t, "[") || strings.HasPrefix(t, "{") {
			var parsed any
			if err := json.Unmarshal([]byte(s), &parsed); err != nil {
				return []any{map[string]any{"type": "text", "text": s}}
			}
			v = parsed
		} else {
			return []any{map[string]any{"type": "text", "text": s}}
		}
	}
	if s, ok := str(v); ok {
		return []any{map[string]any{"type": "text", "text": s}}
	}
	if items, ok := v.([]any); ok {
		out := make([]any, 0, len(items))
		for _, p := range items {
			out = append(out, hermesPart(p))
		}
		return out
	}
	if vm := m(v); vm != nil {
		if txt, ok := str(vm["text"]); ok {
			return []any{map[string]any{"type": "text", "text": txt}}
		}
		return []any{hermesDataPart(v)}
	}
	return []any{map[string]any{"type": "text", "text": hermesString(v)}}
}

func hermesResultValue(raw any) any {
	if s, ok := str(raw); ok {
		return s
	}
	return hermesContent(raw)
}

/* ---------------------------------------------------------- tool calls */

type hermesToolCall struct {
	id, name string
	args     any
	argsSet  bool // JS: arguments would be undefined → key dropped
}

// hermesParseToolCalls mirrors parseToolCalls: OpenAI-shaped JSON,
// [{id, function:{name, arguments}}], parsed tolerantly.
func hermesParseToolCalls(raw any, seq int) []hermesToolCall {
	if !hermesTruthy(raw) {
		return nil
	}
	v := raw
	if s, ok := str(raw); ok {
		var parsed any
		if err := json.Unmarshal([]byte(s), &parsed); err != nil {
			return nil
		}
		v = parsed
	}
	items, ok := v.([]any)
	if !ok {
		return nil
	}
	out := []hermesToolCall{}
	for i, tc := range items {
		tcm := m(tc)
		var fnAny any // fn = tc?.function ?? tc ?? {}
		if tcm != nil {
			fnAny = tcm["function"]
		}
		if fnAny == nil {
			fnAny = tc
		}
		fn := m(fnAny)

		// args = fn.arguments ?? fn.input ?? fn.args
		var args any
		argsSet := false
		if av := fn["arguments"]; av != nil {
			args, argsSet = av, true
		} else if av := fn["input"]; av != nil {
			args, argsSet = av, true
		} else if av, present := fn["args"]; present {
			args, argsSet = av, true // present-but-null keeps the null
		}
		if s, ok := str(args); ok {
			var parsed any
			if err := json.Unmarshal([]byte(s), &parsed); err == nil {
				args = parsed
			} // else keep the raw string
		}

		var name any = fn["name"] // fn.name ?? tc?.name
		if name == nil && tcm != nil {
			name = tcm["name"]
		}
		if !hermesTruthy(name) {
			continue
		}

		var idv any // tc?.id ?? tc?.tool_call_id
		if tcm != nil {
			idv = tcm["id"]
			if idv == nil {
				idv = tcm["tool_call_id"]
			}
		}
		id := fmt.Sprintf("call_%d_%d", seq, i)
		if idv != nil {
			id = hermesString(idv)
		}
		out = append(out, hermesToolCall{id: id, name: hermesString(name), args: args, argsSet: argsSet})
	}
	return out
}

/* --------------------------------------------------------- the builder */

func buildHermesRun(session map[string]any, messages []any) map[string]any {
	var name any = "hermes session" // title || display_name || id || fallback
	for _, cand := range []any{session["title"], session["display_name"], session["id"]} {
		if hermesTruthy(cand) {
			name = cand
			break
		}
	}
	startedAt := hermesIsoSec(session["started_at"])
	var system any // session.system_prompt || undefined
	if hermesTruthy(session["system_prompt"]) {
		system = session["system_prompt"]
	}
	var provider any = "unknown" // session.billing_provider || "unknown"
	if hermesTruthy(session["billing_provider"]) {
		provider = session["billing_provider"]
	}
	modelAny := session["model"] // session.model ?? "unknown"
	if modelAny == nil {
		modelAny = "unknown"
	}
	modelID := hermesString(modelAny)
	providerStr := hermesString(provider)

	root := map[string]any{
		"id": "root", "parent_id": nil, "type": "agent",
		"name": name, "started_at": startedAt,
	}
	spans := []any{root}

	pending := []any{}
	seq := 0
	prevTs := session["started_at"]
	lastTs := session["started_at"]
	openTools := map[string]map[string]any{}

	for _, mv := range messages {
		mm := m(mv)
		roleStr, _ := str(mm["role"])
		ts := mm["timestamp"]
		switch roleStr {
		case "system":
			prevTs = ts
			continue // captured via session.system_prompt
		case "user":
			pending = append(pending, map[string]any{"role": "user", "content": hermesContent(mm["content"])})
			lastTs = ts
		case "tool":
			id, idIsStr := str(mm["tool_call_id"])
			var span map[string]any
			if idIsStr {
				span = openTools[id]
			}
			if span != nil {
				span["ended_at"] = hermesIsoSec(ts)
				span["output"] = map[string]any{"result": hermesResultValue(mm["content"])}
				delete(openTools, id)
			}
			tr := map[string]any{"type": "tool_result", "content": hermesContent(mm["content"])}
			if v, present := mm["tool_call_id"]; present {
				tr["tool_call_id"] = v
			}
			pending = append(pending, map[string]any{"role": "tool", "content": []any{tr}})
			lastTs = ts
		case "assistant":
			toolCalls := hermesParseToolCalls(mm["tool_calls"], seq)
			outContent := []any{}
			reasoning := mm["reasoning"] // m.reasoning || m.reasoning_content
			if !hermesTruthy(reasoning) {
				reasoning = mm["reasoning_content"]
			}
			if hermesTruthy(reasoning) {
				outContent = append(outContent, map[string]any{"type": "thinking", "text": hermesString(reasoning)})
			}
			outContent = append(outContent, hermesContent(mm["content"])...)
			for _, tc := range toolCalls {
				part := map[string]any{"type": "tool_call", "id": tc.id, "name": tc.name}
				if tc.argsSet {
					part["arguments"] = tc.args
				}
				outContent = append(outContent, part)
			}

			status := "ok"
			if fr, ok := str(mm["finish_reason"]); ok && fr == "error" {
				status = "error"
			}
			input := map[string]any{"messages": pending}
			if system != nil { // ...(system ? { system } : {})
				input["system"] = system
			}
			seq++
			span := map[string]any{
				"id":         fmt.Sprintf("s%d", seq),
				"parent_id":  "root",
				"type":       "llm_call",
				"name":       "turn",
				"started_at": hermesIsoSec(prevTs),
				"ended_at":   hermesIsoSec(ts),
				"status":     status,
				"model":      map[string]any{"id": modelID, "provider": providerStr},
				"input":      input,
				"output": map[string]any{"messages": []any{
					map[string]any{"role": "assistant", "content": outContent},
				}},
			}
			if v, present := mm["token_count"]; present && v != nil {
				span["usage"] = map[string]any{"output_tokens": v}
			}
			spans = append(spans, span)
			pending = []any{}

			for _, tc := range toolCalls {
				seq++
				tin := map[string]any{"name": tc.name, "tool_call_id": tc.id}
				if tc.argsSet {
					tin["arguments"] = tc.args
				}
				toolSpan := map[string]any{
					"id":         fmt.Sprintf("s%d", seq),
					"parent_id":  span["id"],
					"type":       "tool_call",
					"name":       tc.name,
					"started_at": hermesIsoSec(ts),
					"status":     "ok",
					"input":      tin,
				}
				spans = append(spans, toolSpan)
				openTools[tc.id] = toolSpan
			}
			lastTs = ts
		}
		prevTs = ts
	}

	endTs := session["ended_at"] // session.ended_at ?? lastTs
	if endTs == nil {
		endTs = lastTs
	}
	root["ended_at"] = hermesIsoSec(endTs)
	root["status"] = "ok"
	if len(pending) > 0 {
		seq++
		spans = append(spans, map[string]any{
			"id":         fmt.Sprintf("s%d", seq),
			"parent_id":  "root",
			"type":       "custom",
			"name":       "trailing message(s) — no assistant reply",
			"started_at": hermesIsoSec(lastTs),
			"ended_at":   hermesIsoSec(lastTs),
			"input":      map[string]any{"messages": pending},
		})
	}

	metadata := map[string]any{}
	if v, present := session["id"]; present {
		metadata["session_id"] = v
	}
	if hermesTruthy(session["cwd"]) {
		metadata["cwd"] = session["cwd"]
	}
	if hermesTruthy(session["source"]) {
		metadata["origin"] = session["source"]
	}

	return map[string]any{
		"schema":     "session/v0",
		"name":       name,
		"created_at": startedAt,
		"source": map[string]any{
			"kind": "import", "harness": "hermes",
			"label": "session-import@0.1.0", "fidelity": "reconstructed",
		},
		"metadata": metadata,
		"spans":    spans,
	}
}
