// dsh (DeepSeek Harness) session logs → session/v0. dsh keeps a session as an
// append-only log: a header line, then one event per line with its sequence
// number and time. The events dsh builds the model's history from are the
// conversation. System, developer and user messages accumulate as the input
// of the next step, each assistant message is an llm_call, and a tool/call
// event opens the tool_call span its tool/result closes. Events that only
// steer the harness (policies, inbox splices, request headers) are not
// replayed.
package importers

import (
	"encoding/base64"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"regexp"
	"strings"
)

func init() { Registry["dsh"] = importDsh }

// dshFormats are the session format versions read here. v4 is what dsh 0.2
// writes. v3, the format of the 0.1 releases, records a tool result inside a
// wrapper block and is otherwise read the same way. dsh refuses a log whose
// version it does not know rather than guess at it, and so does this.
var dshFormats = map[int]bool{3: true, 4: true}

// dshAttachment is the name dsh gives stored bytes: the SHA-256 of them.
var dshAttachment = regexp.MustCompile(`^sha256:([0-9a-f]{64})$`)

func dshDataPart(data any) map[string]any {
	return map[string]any{"type": "data", "data": data}
}

// dshArguments is a tool call's arguments. dsh keeps the JSON text the model
// produced; text that is not JSON stays as it was written.
func dshArguments(raw any) any {
	text, ok := str(raw)
	if !ok {
		return raw
	}
	var parsed any
	if json.Unmarshal([]byte(text), &parsed) != nil {
		return text
	}
	return parsed
}

// dshContent maps the blocks of one message. dsh keeps an image's bytes in
// its attachment store and a reference in the log; an image whose bytes are
// not at hand, like any block this does not know, is kept as recorded.
func dshContent(blocks []any, attachments string) []any {
	parts := []any{}
	for _, bv := range blocks {
		block := m(bv)
		switch strOr(block["type"], "") {
		case "text":
			parts = append(parts, map[string]any{"type": "text", "text": strOr(block["text"], "")})
		case "reasoning":
			parts = append(parts, map[string]any{"type": "thinking", "text": strOr(block["text"], "")})
		case "tool-call":
			part := map[string]any{"type": "tool_call", "arguments": dshArguments(block["arguments"])}
			if id, ok := str(block["id"]); ok {
				part["id"] = id
			}
			if name, ok := str(block["name"]); ok {
				part["name"] = name
			}
			parts = append(parts, part)
		case "image":
			ref := m(block["attachment"])
			if image, ok := dshImage(strOr(ref["attachmentId"], ""), strOr(ref["mediaType"], ""), attachments); ok {
				parts = append(parts, image)
			} else {
				parts = append(parts, dshDataPart(bv))
			}
		default:
			parts = append(parts, dshDataPart(bv))
		}
	}
	return parts
}

// dshImage reads an image out of dsh's attachment store, which files each
// object under the first two characters of its hash.
func dshImage(id, mime, attachments string) (map[string]any, bool) {
	match := dshAttachment.FindStringSubmatch(id)
	if match == nil || attachments == "" {
		return nil, false
	}
	file := filepath.Join(attachments, match[1][:2], match[1])
	info, err := os.Stat(file)
	if err != nil || info.Size() > 32<<20 {
		return nil, false
	}
	raw, err := os.ReadFile(file)
	if err != nil {
		return nil, false
	}
	if !strings.HasPrefix(mime, "image/") {
		mime = ""
	}
	return imagePart(base64.StdEncoding.EncodeToString(raw), mime)
}

// dshResult is what a tool returned: its text when that is all there is, or
// its parts when it also returned an image.
func dshResult(parts []any) any {
	texts := []string{}
	for _, pv := range parts {
		part := m(pv)
		if strOr(part["type"], "") != "text" {
			return parts
		}
		texts = append(texts, strOr(part["text"], ""))
	}
	return strings.Join(texts, "\n")
}

func dshUsage(raw any) map[string]any {
	recorded := m(raw)
	if recorded == nil {
		return nil
	}
	usage := map[string]any{}
	for from, to := range map[string]string{"inputTokens": "input_tokens", "outputTokens": "output_tokens", "totalTokens": "total_tokens",
		"cacheReadTokens": "cache_read_tokens", "cacheWriteTokens": "cache_write_tokens", "reasoningTokens": "reasoning_tokens"} {
		if n, ok := recorded[from].(float64); ok && n >= 0 {
			usage[to] = n
		}
	}
	return usage
}

// dshOwnEvents drops the history a forked sub-agent starts with. dsh seeds
// the fork with a copy of the parent's completed turns and marks where its
// own work begins; the copy is already in the parent this is nested under.
func dshOwnEvents(events []map[string]any) ([]map[string]any, int) {
	cut := -1
	for i, event := range events {
		if strOr(event["type"], "") == "session/end-seed" && m(event["data"])["inherited"] == true {
			cut = i
		}
	}
	return events[cut+1:], cut + 1
}

func importDsh(in Input) (map[string]any, error) {
	var header map[string]any
	var events []map[string]any
	for i, line := range in.Lines {
		var entry map[string]any
		if json.Unmarshal([]byte(line), &entry) != nil {
			continue // dsh discards a line it was cut off while writing
		}
		if i == 0 && strOr(entry["type"], "") == "session" {
			header = entry
			continue
		}
		events = append(events, entry)
	}
	if header == nil {
		return nil, fmt.Errorf("not a dsh session log: it does not start with a session header")
	}
	version := int(numOr(header["version"], -1))
	if !dshFormats[version] {
		return nil, fmt.Errorf("dsh session format v%d is not one slink reads (v3 and v4 are)", version)
	}
	inherited := 0
	if in.SubAgent {
		events, inherited = dshOwnEvents(events)
	}

	created := header["createdAt"]
	root := map[string]any{"id": "root", "parent_id": nil, "type": "agent", "started_at": ocISO(created)}
	spans := []any{root}
	seq := 0
	next := func() string {
		seq++
		return fmt.Sprintf("s%d", seq)
	}

	title, prompt := "", ""
	pending := []any{}
	lastTime, callStart := created, created
	lastCall := ""                      // the llm_call the open step belongs to
	open := map[string]map[string]any{} // tool call id -> its span, until the result arrives
	var delegations []string            // delegation calls still running, oldest first
	agentCalls := map[string]string{}   // sub-agent session id -> the call that started it
	model := map[string]any{}
	compactions, retries := 0, 0

	for _, event := range events {
		data := m(event["data"])
		at := event["time"]
		if _, timed := at.(float64); timed {
			lastTime = at
		} else {
			at = lastTime
		}
		switch strOr(event["type"], "") {
		case "session/title":
			if t := strings.TrimSpace(strOr(data["title"], "")); t != "" && strOr(m(data["source"])["kind"], "") != "fallback" {
				title = t
			}
		case "request/context":
			model = data
		case "step/start", "llm/retry-started":
			callStart = at
		case "llm/retry":
			retries++
		case "compaction/end":
			compactions++

		case "system/message", "developer/message":
			message := m(data["message"])
			pending = append(pending, map[string]any{"role": strOr(message["role"], "system"), "content": dshContent(arr(message["content"]), in.Blobs)})
		case "user/message":
			// dsh says who wrote each message. Only the person's own are
			// theirs; the rest is context the harness supplied in their turn.
			role := "system"
			if strOr(m(data["source"])["kind"], "") == "user" {
				role = "user"
				if prompt == "" {
					for _, part := range arr(data["content"]) {
						if text := strings.TrimSpace(strOr(m(part)["text"], "")); text != "" {
							prompt = text
							break
						}
					}
				}
			}
			pending = append(pending, map[string]any{"role": role, "content": dshContent(arr(data["content"]), in.Blobs)})

		case "assistant/attempt":
			// A model request that failed before it produced a message. dsh
			// keeps the attempt, and its retry is the call that follows. The
			// messages waiting were sent with the first of them.
			span := map[string]any{
				"id": next(), "parent_id": "root", "type": "llm_call", "name": "turn",
				"started_at": ocISO(callStart), "ended_at": ocISO(at), "status": "error",
				"input": map[string]any{"messages": pending}, "output": map[string]any{"messages": []any{}},
				"error": map[string]any{"message": "the model request did not complete"},
			}
			pending = []any{}
			for _, record := range arr(data["stream"]) {
				reason := m(m(m(record)["chunk"])["reason"])
				if failure := m(reason["failure"]); failure != nil {
					message := strOr(failure["message"], strOr(reason["kind"], "error"))
					if code := strOr(failure["code"], ""); code != "" {
						message += " (" + code + ")"
					}
					span["error"] = map[string]any{"message": message}
				}
			}
			dshModel(span, nil, model)
			spans = append(spans, span)
			callStart = at
		case "assistant/message":
			message := m(data["message"])
			span := map[string]any{
				"id": next(), "parent_id": "root", "type": "llm_call", "name": "turn",
				"started_at": ocISO(callStart), "ended_at": ocISO(at), "status": "ok",
				"input": map[string]any{"messages": pending},
				"output": map[string]any{"messages": []any{map[string]any{
					"role": "assistant", "content": dshContent(arr(message["content"]), in.Blobs),
				}}},
			}
			dshModel(span, m(message["source"]), model)
			if usage := dshUsage(data["usage"]); len(usage) > 0 {
				span["usage"] = usage
			}
			if data["interrupted"] == true {
				span["metadata"] = map[string]any{"interrupted": true}
			}
			spans = append(spans, span)
			pending = []any{}
			lastCall = strOr(span["id"], "")
			callStart = at

		case "tool/call":
			id := strOr(data["callId"], "")
			name := strOr(data["name"], "tool")
			parent := lastCall
			if parent == "" {
				parent = "root"
			}
			span := map[string]any{
				"id": next(), "parent_id": parent, "type": "tool_call", "name": name,
				"started_at": ocISO(at), "ended_at": ocISO(at),
				"input": map[string]any{"tool_call_id": id, "name": name, "arguments": dshArguments(data["arguments"])},
			}
			spans = append(spans, span)
			if id != "" {
				open[id] = span
				if name == "subagent" || name == "subagent_fork" {
					delegations = append(delegations, id)
				}
			}
		case "subagent/catalog":
			// The parent's note that a sub-agent now exists. It does not name
			// the call that asked for it; that is the delegation still
			// waiting, and the oldest of them when several are.
			if child := strOr(data["childId"], ""); child != "" && len(delegations) > 0 {
				agentCalls[child] = delegations[0]
				delegations = delegations[1:]
			}
		case "tool/result":
			message := m(data["message"])
			blocks := arr(message["content"])
			id := strOr(message["toolCallId"], strOr(m(message["source"])["callId"], ""))
			failed := message["isError"] == true
			// v3 wraps the result in one block that carries the call's id.
			if wrapper := m(first(blocks)); len(blocks) == 1 && strOr(wrapper["type"], "") == "tool-result" {
				blocks = arr(wrapper["content"])
				id = strOr(wrapper["toolCallId"], id)
				failed = wrapper["isError"] == true
			}
			span := open[id]
			if span == nil {
				span = map[string]any{"id": next(), "parent_id": "root", "type": "tool_call", "name": "tool",
					"started_at": ocISO(at), "input": map[string]any{"tool_call_id": id, "name": "tool"}}
				spans = append(spans, span)
			}
			delete(open, id)
			for i, waiting := range delegations {
				if waiting == id {
					delegations = append(delegations[:i], delegations[i+1:]...)
					break
				}
			}
			output := map[string]any{"result": dshResult(dshContent(blocks, in.Blobs))}
			span["status"] = "ok"
			if failed || data["error"] != nil {
				output["is_error"] = true
				span["status"] = "error"
			}
			span["output"] = output
			span["ended_at"] = ocISO(at)
		}
	}

	name := title
	if name == "" {
		name = prompt
		if cut := []rune(name); len(cut) > 80 {
			name = string(cut[:80]) + "…"
		}
	}
	if name == "" {
		name = strOr(header["id"], in.Fallback)
	}
	if name == "" {
		name = "dsh session"
	}
	root["name"] = name
	root["ended_at"] = ocISO(lastTime)
	root["status"] = "ok"
	if len(pending) > 0 {
		spans = append(spans, map[string]any{
			"id": next(), "parent_id": "root", "type": "custom",
			"name":       "trailing message(s) — no assistant reply",
			"started_at": ocISO(lastTime), "ended_at": ocISO(lastTime),
			"input": map[string]any{"messages": pending},
		})
	}

	metadata := map[string]any{"session_format": version}
	if id, ok := str(header["id"]); ok {
		metadata["session_id"] = id
	}
	if cwd, ok := str(header["cwd"]); ok {
		metadata["cwd"] = cwd
	}
	if parent, ok := str(header["parentSession"]); ok {
		metadata["parent_session_id"] = parent
	}
	if preset, ok := str(header["agentPreset"]); ok {
		metadata["agent_preset"] = preset
	}
	if inherited > 0 {
		metadata["inherited_events_skipped"] = inherited
	}
	if compactions > 0 {
		metadata["compactions"] = compactions
	}
	if retries > 0 {
		metadata["model_retries"] = retries
	}

	run := map[string]any{
		"schema":     "session/v0",
		"name":       name,
		"created_at": ocISO(created),
		"source": map[string]any{
			"kind": "import", "harness": "dsh",
			"label": "session-import@0.1.0", "fidelity": "reconstructed",
		},
		"metadata": metadata,
		"spans":    spans,
	}
	adoptAgents(run, "dsh", in.Agents, agentCalls)
	return run, nil
}

// dshModel names the model of a call: the one the reply says it came from,
// or the one the request was prepared for.
func dshModel(span, source, request map[string]any) {
	model := map[string]any{"id": strOr(source["model"], strOr(request["model"], "unknown"))}
	if provider := strOr(source["provider"], strOr(request["provider"], "")); provider != "" {
		model["provider"] = provider
	}
	span["model"] = model
}

func first(items []any) any {
	if len(items) == 0 {
		return nil
	}
	return items[0]
}
