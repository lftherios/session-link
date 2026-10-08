// Claude Code importer: port of transcriptToRun in cli/import-session.mjs.
// Walks a ~/.claude/projects JSONL transcript and reconstructs a session/v0
// run: user/tool messages accumulate as the pending delta, each model
// response becomes an llm_call whose input is that delta, tool_use blocks
// become tool_call spans closed by the matching tool_result. Only what the
// person typed keeps the user role: tool results become tool messages, and
// isMeta rows and compaction summaries become system messages. Synthetic API error entries, such as
// rate limits, become failed custom spans instead of agent responses.
package importers

import (
	"encoding/json"
	"fmt"
	"strings"

	"github.com/lftherios/session-link/internal/normalize"
)

func init() { Registry["claude-code"] = claudeCodeImport }

const ccImportLabel = "session-import@0.1.0"

func claudeCodeImport(in Input) (map[string]any, error) {
	run, agentCalls := ccTranscriptToRun(in.Lines, in.Fallback, in.SubAgent)
	if run != nil {
		adoptAgents(run, "claude-code", in.Agents, agentCalls)
	}
	return run, nil
}

// ccFirstUserText extracts the first real user message's text, clipped to
// title length — the human name for an otherwise UUID-named session.
func ccFirstUserText(entries []map[string]any) string {
	for _, e := range entries {
		if strOr(e["type"], "") != "user" || ccTruthy(e["isMeta"]) || ccTruthy(e["isCompactSummary"]) {
			continue // injected caveats, context and compaction summaries are not the ask
		}
		msg := m(e["message"])
		var text string
		switch c := msg["content"].(type) {
		case string:
			text = c
		case []any:
			for _, p := range c {
				part := m(p)
				if strOr(part["type"], "") == "text" {
					text = strOr(part["text"], "")
					break
				}
			}
		}
		text = strings.Join(strings.Fields(text), " ") // collapse newlines/runs
		if text == "" || strings.HasPrefix(text, "<") || strings.HasPrefix(text, "Caveat:") {
			continue // tool results / injected caveats — not a title
		}
		// Clip only as a safety net against pasted walls of text — narrow
		// surfaces (list, gate) apply their own tighter budgets, and the
		// viewer gets the whole thing.
		if r := []rune(text); len(r) > 200 {
			return string(r[:199]) + "…"
		}
		return text
	}
	return ""
}

// ccTruthy mirrors JS truthiness for the `if (x)` sites in the JS mapper.
func ccTruthy(v any) bool {
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
	return true // objects and arrays are always truthy
}

// ccString mirrors String(x) for the model-id site (String(msg.model ?? "unknown")).
func ccString(v any) string {
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
	default:
		return fmt.Sprintf("%v", x) // JS would give [object Object]; unreachable for real transcripts
	}
}

// ccUserRole keeps the user role for what the person typed. Tool results
// become tool messages, as in the pi and Hermes importers. isMeta rows
// (injected caveats and notices) and the summary Claude Code writes in the
// person's place when it compacts a conversation become system messages.
func ccUserRole(e map[string]any, content []any) string {
	results := len(content) > 0
	for _, pv := range content {
		if strOr(m(pv)["type"], "") != "tool_result" {
			results = false
			break
		}
	}
	switch {
	case results:
		return "tool"
	case ccTruthy(e["isMeta"]), ccTruthy(e["isCompactSummary"]):
		return "system"
	}
	return "user"
}

// ccText joins the text parts of normalized content, or returns fallback.
func ccText(content []any, fallback string) string {
	var texts []string
	for _, pv := range content {
		p := m(pv)
		if t := strOr(p["text"], ""); strOr(p["type"], "") == "text" && t != "" {
			texts = append(texts, t)
		}
	}
	if len(texts) == 0 {
		return fallback
	}
	return strings.Join(texts, "\n\n")
}

// ccPut copies src[srcKey] into dst[key] only when the key is present —
// mirroring JSON.stringify's omission of properties assigned `undefined`.
func ccPut(dst map[string]any, key string, src map[string]any, srcKey string) {
	if v, ok := src[srcKey]; ok {
		dst[key] = v
	}
}

// ccTranscriptToRun also returns, for each sub-agent the session started, the
// tool call that started it. A sub-agent's own transcript is all sidechain
// entries, which a session's transcript does not replay; subAgent reads them.
func ccTranscriptToRun(lines []string, sessionName string, subAgent bool) (map[string]any, map[string]string) {
	agentCalls := map[string]string{} // agent id -> tool_use id
	entries := []map[string]any{}
	skippedSidechain := 0
	var name any = sessionName
	for _, line := range lines {
		var ev any
		if err := json.Unmarshal([]byte(line), &ev); err != nil {
			continue
		}
		e := m(ev)
		if strOr(e["type"], "") == "summary" && ccTruthy(e["summary"]) {
			name = e["summary"]
		}
		// Claude Code records its own session title; the latest one names it.
		if strOr(e["type"], "") == "ai-title" && ccTruthy(e["aiTitle"]) {
			name = e["aiTitle"]
		}
		t := strOr(e["type"], "")
		if t != "user" && t != "assistant" {
			continue
		}
		if ccTruthy(e["isSidechain"]) && !subAgent {
			skippedSidechain++
			continue
		}
		if !ccTruthy(e["message"]) {
			continue
		}
		entries = append(entries, e)
	}
	if len(entries) == 0 && skippedSidechain > 0 {
		// Nothing but sidechain entries: this is a sub-agent's own transcript.
		return ccTranscriptToRun(lines, sessionName, true)
	}
	if len(entries) == 0 {
		return nil, nil
	}
	// No summary in the transcript? A clipped first user message beats the
	// raw session UUID everywhere a human sees the name (list, the publish
	// gate, the published page title).
	if s, _ := name.(string); s == sessionName {
		if t := ccFirstUserText(entries); t != "" {
			name = t
		}
	}

	first := entries[0]
	last := entries[len(entries)-1]
	root := map[string]any{
		"id":        "root",
		"parent_id": nil,
		"type":      "agent",
		"name":      name,
		"status":    "ok",
	}
	ccPut(root, "started_at", first, "timestamp")
	ccPut(root, "ended_at", last, "timestamp")
	spans := []any{root}

	// Walk turns: user/tool messages accumulate as the pending delta; each
	// model response becomes an llm_call whose input is that delta. tool_use
	// blocks become tool_call spans, closed by the matching tool_result.
	pending := []any{}
	var pendingStart any // timestamp of the first message in the pending delta
	prevTs := first["timestamp"]
	openTools := map[string]map[string]any{} // tool_use_id -> span
	seq := 0
	var call, reply map[string]any // the latest llm_call and its assistant message
	callID := ""                   // that call's message id

	for _, e := range entries {
		msg := m(e["message"])
		if strOr(e["type"], "") == "user" {
			content := normalize.AnthropicContentWithImages(msg["content"])
			for _, pv := range content {
				part := m(pv)
				if strOr(part["type"], "") != "tool_result" {
					continue
				}
				id, _ := str(part["tool_call_id"])
				toolSpan := openTools[id]
				if toolSpan != nil {
					toolSpan["ended_at"] = e["timestamp"]
					result := e["toolUseResult"] // ?? falls through on both null and absent
					if result == nil {
						result = part["content"]
					}
					output := map[string]any{"result": result}
					if ccTruthy(part["is_error"]) {
						output["is_error"] = true
						toolSpan["status"] = "error"
					}
					toolSpan["output"] = output
					if agent := strOr(m(e["toolUseResult"])["agentId"], ""); agent != "" {
						agentCalls[agent] = id
					}
					delete(openTools, id)
				}
			}
			if pendingStart == nil { // ??=
				pendingStart = e["timestamp"]
			}
			pending = append(pending, map[string]any{"role": ccUserRole(e, content), "content": content})
		} else if ccTruthy(e["isApiErrorMessage"]) {
			// Claude Code writes harness failures as assistant entries from a
			// synthetic model. No model produced them, so keep the text as a
			// failed span rather than an llm_call with an agent response.
			seq++
			failure := map[string]any{"message": ccText(normalize.AnthropicContentWithImages(msg["content"]), "Claude Code reported an API error")}
			if kind := strOr(e["error"], ""); kind != "" {
				failure["type"] = kind
			}
			spans = append(spans, map[string]any{
				"id":         fmt.Sprintf("s%d", seq),
				"parent_id":  "root",
				"type":       "custom",
				"name":       "harness error",
				"started_at": prevTs,
				"ended_at":   e["timestamp"],
				"status":     "error",
				"error":      failure,
				"input":      map[string]any{"messages": pending},
			})
			pending = []any{}
			pendingStart = nil
			call = nil
		} else {
			// Claude Code writes one entry per content block of a response, all
			// carrying the response's message id and usage. They are one model
			// call: later blocks join the reply, and the last usage stands.
			id, _ := str(msg["id"])
			span := call
			if span == nil || id == "" || id != callID {
				seq++
				modelID := "unknown"
				if msg["model"] != nil {
					modelID = ccString(msg["model"])
				}
				reply = map[string]any{"role": "assistant", "content": normalize.AnthropicContentWithImages(msg["content"])}
				span = map[string]any{
					"id":         fmt.Sprintf("s%d", seq),
					"parent_id":  "root",
					"type":       "llm_call",
					"name":       "turn",
					"started_at": prevTs,
					"ended_at":   e["timestamp"],
					"status":     "ok",
					"model":      map[string]any{"id": modelID, "provider": "anthropic"},
					"input":      map[string]any{"messages": pending},
					"output":     map[string]any{"messages": []any{reply}},
				}
				spans = append(spans, span)
				pending = []any{}
				pendingStart = nil
				call, callID = span, id
			} else {
				reply["content"] = append(arr(reply["content"]), normalize.AnthropicContentWithImages(msg["content"])...)
				span["ended_at"] = e["timestamp"]
			}
			if usage := normalize.AnthropicUsage(msg["usage"]); usage != nil {
				span["usage"] = usage
			}
			for _, bv := range arr(msg["content"]) {
				block := m(bv)
				if strOr(block["type"], "") != "tool_use" {
					continue
				}
				id, okID := str(block["id"])
				if !okID {
					continue
				}
				seq++
				input := map[string]any{"tool_call_id": id}
				ccPut(input, "name", block, "name")
				ccPut(input, "arguments", block, "input")
				toolSpan := map[string]any{
					"id":         fmt.Sprintf("s%d", seq),
					"parent_id":  span["id"],
					"type":       "tool_call",
					"started_at": e["timestamp"],
					"status":     "ok",
					"input":      input,
				}
				ccPut(toolSpan, "name", block, "name")
				spans = append(spans, toolSpan)
				openTools[id] = toolSpan
			}
		}
		prevTs = e["timestamp"]
	}

	// A transcript can end on a user turn (no assistant reply yet). Raw is
	// sacred: keep those messages as a custom span instead of dropping them —
	// fabricating an llm_call with no response would be dishonest.
	if len(pending) > 0 {
		seq++
		startedAt := pendingStart
		if startedAt == nil { // ??
			startedAt = prevTs
		}
		spans = append(spans, map[string]any{
			"id":         fmt.Sprintf("s%d", seq),
			"parent_id":  "root",
			"type":       "custom",
			"name":       "trailing user message(s) — transcript ends before a reply",
			"started_at": startedAt,
			"ended_at":   prevTs,
			"input":      map[string]any{"messages": pending},
		})
	}

	metadata := map[string]any{}
	ccPut(metadata, "session_id", first, "sessionId")
	ccPut(metadata, "cwd", first, "cwd")
	if ccTruthy(first["version"]) {
		metadata["harness_version"] = first["version"]
	}
	if skippedSidechain != 0 {
		metadata["sidechain_entries_skipped"] = skippedSidechain
	}

	run := map[string]any{
		"schema": "session/v0",
		"name":   name,
		"source": map[string]any{
			"kind": "import", "harness": "claude-code",
			"label": ccImportLabel, "fidelity": "reconstructed",
		},
		"metadata": metadata,
		"spans":    spans,
	}
	ccPut(run, "created_at", first, "timestamp")
	return run, agentCalls
}
