// Claude Code importer: port of transcriptToRun in cli/import-session.mjs.
// Walks a ~/.claude/projects JSONL transcript and reconstructs a session/v0
// run: user/tool messages accumulate as the pending delta, each assistant
// entry becomes an llm_call whose input is that delta, tool_use blocks
// become tool_call spans closed by the matching tool_result.
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
	return ccTranscriptToRun(in.Lines, in.Fallback), nil
}

// ccFirstUserText extracts the first real user message's text, clipped to
// title length — the human name for an otherwise UUID-named session.
func ccFirstUserText(entries []map[string]any) string {
	for _, e := range entries {
		if strOr(e["type"], "") != "user" || ccTruthy(e["isMeta"]) {
			continue // isMeta rows are injected caveats/context, not the ask
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

// ccPut copies src[srcKey] into dst[key] only when the key is present —
// mirroring JSON.stringify's omission of properties assigned `undefined`.
func ccPut(dst map[string]any, key string, src map[string]any, srcKey string) {
	if v, ok := src[srcKey]; ok {
		dst[key] = v
	}
}

func ccTranscriptToRun(lines []string, sessionName string) map[string]any {
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
		t := strOr(e["type"], "")
		if t != "user" && t != "assistant" {
			continue
		}
		if ccTruthy(e["isSidechain"]) {
			skippedSidechain++
			continue
		}
		if !ccTruthy(e["message"]) {
			continue
		}
		entries = append(entries, e)
	}
	if len(entries) == 0 {
		return nil
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
	// assistant entry becomes an llm_call whose input is that delta. tool_use
	// blocks become tool_call spans, closed by the matching tool_result.
	pending := []any{}
	var pendingStart any // timestamp of the first message in the pending delta
	prevTs := first["timestamp"]
	openTools := map[string]map[string]any{} // tool_use_id -> span
	seq := 0

	for _, e := range entries {
		msg := m(e["message"])
		if strOr(e["type"], "") == "user" {
			content := normalize.AnthropicContent(msg["content"])
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
					delete(openTools, id)
				}
			}
			if pendingStart == nil { // ??=
				pendingStart = e["timestamp"]
			}
			pending = append(pending, map[string]any{"role": "user", "content": content})
		} else {
			seq++
			modelID := "unknown"
			if msg["model"] != nil {
				modelID = ccString(msg["model"])
			}
			span := map[string]any{
				"id":         fmt.Sprintf("s%d", seq),
				"parent_id":  "root",
				"type":       "llm_call",
				"name":       "turn",
				"started_at": prevTs,
				"ended_at":   e["timestamp"],
				"status":     "ok",
				"model":      map[string]any{"id": modelID, "provider": "anthropic"},
				"input":      map[string]any{"messages": pending},
				"output": map[string]any{"messages": []any{map[string]any{
					"role": "assistant", "content": normalize.AnthropicContent(msg["content"]),
				}}},
			}
			if usage := normalize.AnthropicUsage(msg["usage"]); usage != nil {
				span["usage"] = usage
			}
			spans = append(spans, span)
			pending = []any{}
			pendingStart = nil
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
	return run
}
