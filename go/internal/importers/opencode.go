// Port of cli/import-opencode.mjs buildOpencodeRun — opencode (sst/opencode)
// session/message/part rows → session/v0. Faithful to the JS reference: same
// field picks, ?? vs || distinctions, span id sequence, timestamp handling,
// and open-world tolerance; parity is pinned by testdata/import/opencode.
package importers

import (
	"encoding/json"
	"fmt"
	"strings"
	"time"
)

func init() { Registry["opencode"] = importOpencode }

/* ------------------------------------------------------------- js helpers */

// ocTruthy mirrors JS Boolean(x) for decoded JSON values.
func ocTruthy(v any) bool {
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
	return true // objects and arrays (even empty) are truthy
}

// ocString mirrors JS String(x) for decoded JSON values.
func ocString(v any) string {
	switch x := v.(type) {
	case nil:
		return "" // unreachable: all call sites coalesce before String()
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
		parts := make([]string, 0, len(x))
		for _, e := range x {
			if e == nil {
				parts = append(parts, "") // join renders null/undefined as ""
			} else {
				parts = append(parts, ocString(e))
			}
		}
		return strings.Join(parts, ",")
	default:
		return "[object Object]"
	}
}

// ocISO mirrors iso(ms): new Date(typeof ms === "number" ? ms : 0).toISOString().
func ocISO(v any) string {
	ms, ok := v.(float64)
	if !ok {
		ms = 0
	}
	return time.UnixMilli(int64(ms)).UTC().Format("2006-01-02T15:04:05.000Z")
}

func ocDataPart(data any) map[string]any {
	return map[string]any{"type": "data", "data": data}
}

/* --------------------------------------------------------------- content */

// ocFilePart maps a file part: an image opencode recorded as a data URL
// becomes an image, and any other file is kept as recorded.
func ocFilePart(part any) any {
	p := m(part)
	if strings.HasPrefix(strOr(p["mime"], ""), "image/") {
		if image, ok := imageURLPart(strOr(p["url"], "")); ok {
			return image
		}
	}
	return ocDataPart(part)
}

// ocUserContent maps the parts of a user message. opencode marks text it
// wrote itself as synthetic, such as its account of reading an attached file;
// that is returned apart, as context, from what the person typed and attached.
func ocUserContent(parts []any) (typed, injected []any) {
	typed = []any{}
	for _, pv := range parts {
		p := m(pv)
		if t, ok := str(p["type"]); ok && t == "text" {
			text := p["text"]
			if text == nil {
				text = ""
			}
			part := map[string]any{"type": "text", "text": text}
			if p["synthetic"] == true {
				injected = append(injected, part)
			} else {
				typed = append(typed, part)
			}
		} else if t == "file" {
			typed = append(typed, ocFilePart(pv))
		} else if ocTruthy(p["type"]) {
			typed = append(typed, ocDataPart(pv)) // keep, don't drop
		}
	}
	return typed, injected
}

// ocAssistantContent mirrors assistantContent(parts): text, reasoning→thinking,
// tool→tool_call, and a file the model produced. step-start / step-finish /
// patch are structural — skipped.
func ocAssistantContent(parts []any) []any {
	out := []any{}
	for _, pv := range parts {
		p := m(pv)
		t, _ := str(p["type"])
		switch t {
		case "text", "reasoning":
			text := p["text"]
			if text == nil {
				text = ""
			}
			kind := "text"
			if t == "reasoning" {
				kind = "thinking"
			}
			out = append(out, map[string]any{"type": kind, "text": text})
		case "file":
			out = append(out, ocFilePart(pv))
		case "tool":
			callID, isStr := str(p["callID"])
			if !isStr {
				continue
			}
			part := map[string]any{"type": "tool_call", "id": callID}
			// name: p.tool / arguments: p.state?.input — undefined keys are
			// dropped by JSON.stringify in JS, so only set when present.
			if v, ok := p["tool"]; ok {
				part["name"] = v
			}
			if st := m(p["state"]); st != nil {
				if v, ok := st["input"]; ok {
					part["arguments"] = v
				}
			}
			out = append(out, part)
		}
	}
	return out
}

// ocUsage mirrors ocUsage(tokens); nil means "undefined" (key omitted).
func ocUsage(tokens any) map[string]any {
	if !ocTruthy(tokens) {
		return nil
	}
	t := m(tokens)
	out := map[string]any{}
	if v, ok := t["input"]; ok && v != nil {
		out["input_tokens"] = v
	}
	if v, ok := t["output"]; ok && v != nil {
		out["output_tokens"] = v
	}
	if v, ok := t["reasoning"]; ok && v != nil {
		out["reasoning_tokens"] = v
	}
	if c := m(t["cache"]); c != nil {
		if v, ok := c["read"]; ok && v != nil {
			out["cache_read_tokens"] = v
		}
	}
	if len(out) == 0 {
		return nil
	}
	return out
}

/* ------------------------------------------------------------ the builder */

// importOpencode mirrors buildOpencodeRun(session, messages).
func importOpencode(in Input) (map[string]any, error) {
	session := in.Session
	if session == nil {
		session = map[string]any{}
	}

	// model = safeParse(session.model) ?? {} — objects pass through, strings
	// get JSON.parse'd, anything else degrades to {} (lookups on nil map).
	model := m(session["model"])
	if model == nil {
		if s, ok := str(session["model"]); ok {
			var parsed any
			if json.Unmarshal([]byte(s), &parsed) == nil {
				model = m(parsed)
			}
		}
	}

	// name = session.title || session.id || "opencode session" (|| truthiness)
	name := session["title"]
	if !ocTruthy(name) {
		name = session["id"]
	}
	if !ocTruthy(name) {
		name = "opencode session"
	}
	startedAt := ocISO(session["time_created"])

	root := map[string]any{
		"id": "root", "parent_id": nil, "type": "agent",
		"name": name, "started_at": startedAt,
	}
	spans := []any{root}

	pending := []any{}
	agentCalls := map[string]string{} // child session id -> the task call that started it
	seq := 0
	lastTs := session["time_created"]

	for _, mv := range in.Messages {
		mm := m(mv)
		d := m(mm["data"]) // d = m.data ?? {}
		role, _ := str(d["role"])
		switch role {
		case "user":
			typed, injected := ocUserContent(arr(mm["parts"]))
			if len(injected) > 0 {
				pending = append(pending, map[string]any{"role": "system", "content": injected})
			}
			pending = append(pending, map[string]any{"role": "user", "content": typed})
			lastTs = mm["time_created"]
		case "assistant":
			// created = d.time?.created ?? m.time_created; completed = d.time?.completed ?? created
			var created, completed any
			if tm := m(d["time"]); tm != nil {
				created = tm["created"]
				completed = tm["completed"]
			}
			if created == nil {
				created = mm["time_created"]
			}
			if completed == nil {
				completed = created
			}
			errored := ocTruthy(d["error"]) || d["finish"] == "error"

			modelID := d["modelID"]
			if modelID == nil {
				modelID = model["id"]
			}
			if modelID == nil {
				modelID = "unknown"
			}
			providerID := d["providerID"]
			if providerID == nil {
				providerID = model["providerID"]
			}
			if providerID == nil {
				providerID = "unknown"
			}

			status := "ok"
			if errored {
				status = "error"
			}
			seq++
			span := map[string]any{
				"id":         fmt.Sprintf("s%d", seq),
				"parent_id":  "root",
				"type":       "llm_call",
				"name":       "turn",
				"started_at": ocISO(created),
				"ended_at":   ocISO(completed),
				"status":     status,
				"model": map[string]any{
					"id":       ocString(modelID),
					"provider": ocString(providerID),
				},
				"input": map[string]any{"messages": pending},
				"output": map[string]any{"messages": []any{map[string]any{
					"role": "assistant", "content": ocAssistantContent(arr(mm["parts"])),
				}}},
			}
			if usage := ocUsage(d["tokens"]); usage != nil {
				span["usage"] = usage
			}
			if errored {
				// String(d.error?.message ?? d.error ?? "error")
				var msg any
				if em := m(d["error"]); em != nil {
					msg = em["message"]
				}
				if msg == nil {
					msg = d["error"]
				}
				if msg == nil {
					msg = "error"
				}
				span["error"] = map[string]any{"message": ocString(msg)}
			}
			spans = append(spans, span)
			pending = []any{}

			for _, pv := range arr(mm["parts"]) {
				p := m(pv)
				if t, _ := str(p["type"]); t != "tool" {
					continue
				}
				callID, isStr := str(p["callID"])
				if !isStr {
					continue
				}
				st := m(p["state"]) // st = p.state ?? {} (nil-map lookups behave alike)
				isErr := st["status"] == "error"
				// The task tool records the session of the sub-agent it ran.
				if child := strOr(m(st["metadata"])["sessionId"], ""); child != "" {
					agentCalls[child] = callID
				}

				var startV, endV any
				if tm := m(st["time"]); tm != nil {
					startV = tm["start"]
					endV = tm["end"]
				}
				if startV == nil {
					startV = created
				}
				if endV == nil {
					endV = completed
				}

				input := map[string]any{"tool_call_id": callID}
				if v, ok := p["tool"]; ok {
					input["name"] = v
				}
				if v, ok := st["input"]; ok {
					input["arguments"] = v
				}

				// result: st.output ?? st.error ?? st.metadata?.output ?? null
				result := st["output"]
				if result == nil {
					result = st["error"]
				}
				if result == nil {
					if md := m(st["metadata"]); md != nil {
						result = md["output"]
					}
				}
				// A tool can return files beside its text, such as the image it
				// read. They are part of what the model received.
				if attachments := arr(st["attachments"]); len(attachments) > 0 {
					parts := []any{}
					if text, ok := str(result); ok && text != "" {
						parts = append(parts, map[string]any{"type": "text", "text": text})
					} else if result != nil && !ok {
						parts = append(parts, ocDataPart(result))
					}
					for _, attachment := range attachments {
						parts = append(parts, ocFilePart(attachment))
					}
					result = parts
				}
				output := map[string]any{"result": result}
				if isErr {
					output["is_error"] = true
				}

				toolName := p["tool"]
				if !ocTruthy(toolName) {
					toolName = "tool" // p.tool || "tool"
				}
				toolStatus := "ok"
				if isErr {
					toolStatus = "error"
				}
				seq++
				spans = append(spans, map[string]any{
					"id":         fmt.Sprintf("s%d", seq),
					"parent_id":  span["id"],
					"type":       "tool_call",
					"name":       toolName,
					"started_at": ocISO(startV),
					"ended_at":   ocISO(endV),
					"status":     toolStatus,
					"input":      input,
					"output":     output,
				})
			}
			lastTs = completed
		}
		// other roles (summaries, etc.) are ignored
	}

	root["ended_at"] = ocISO(lastTs)
	root["status"] = "ok"
	if len(pending) > 0 {
		seq++
		spans = append(spans, map[string]any{
			"id":         fmt.Sprintf("s%d", seq),
			"parent_id":  "root",
			"type":       "custom",
			"name":       "trailing message(s) — no assistant reply",
			"started_at": ocISO(lastTs),
			"ended_at":   ocISO(lastTs),
			"input":      map[string]any{"messages": pending},
		})
	}

	metadata := map[string]any{}
	// session_id: session.id — JSON.stringify drops the key only when the
	// value is undefined (absent), so mirror key presence, not truthiness.
	if v, ok := session["id"]; ok {
		metadata["session_id"] = v
	}
	if ocTruthy(session["directory"]) {
		metadata["cwd"] = session["directory"]
	}
	if ocTruthy(session["version"]) {
		metadata["harness_version"] = session["version"]
	}

	run := map[string]any{
		"schema":     "session/v0",
		"name":       name,
		"created_at": startedAt,
		"source": map[string]any{
			"kind": "import", "harness": "opencode",
			"label": "session-import@0.1.0", "fidelity": "reconstructed",
		},
		"metadata": metadata,
		"spans":    spans,
	}
	adoptAgents(run, "opencode", in.Agents, agentCalls)
	return run, nil
}
