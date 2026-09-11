package share

import (
	"fmt"
	"regexp"
	"strings"
	"unicode/utf16"
	"unicode/utf8"

	"github.com/lftherios/session-link/internal/format"
)

type Selection struct {
	ID    string `json:"id"`
	Start *int   `json:"start,omitempty"`
	End   *int   `json:"end,omitempty"`
}

type Draft struct {
	Title   string      `json:"title"`
	Note    string      `json:"note"`
	Items   []Selection `json:"items"`
	Primary string      `json:"primary"`
}

func (d Draft) Validate(cat Catalog, ready bool) error {
	if utf8.RuneCountInString(d.Title) > 256 {
		return fmt.Errorf("keep the title within 256 characters")
	}
	if utf8.RuneCountInString(d.Note) > 10000 {
		return fmt.Errorf("keep the author note within 10,000 characters")
	}
	if len(d.Items) > 1000 {
		return fmt.Errorf("select at most 1,000 pieces of content")
	}
	if ready && len(d.Items) == 0 {
		return fmt.Errorf("select something to include in the excerpt")
	}
	if ready && strings.TrimSpace(d.Title) == "" {
		return fmt.Errorf("give the excerpt a title")
	}
	units := map[string]Unit{}
	for _, u := range cat.Units {
		units[u.ID] = u
	}
	seen := map[string]bool{}
	for _, item := range d.Items {
		u, ok := units[item.ID]
		if !ok || u.Unavailable {
			return fmt.Errorf("selection %q is unavailable; reload the source session", item.ID)
		}
		if seen[item.ID] {
			return fmt.Errorf("selection %q appears more than once", item.ID)
		}
		seen[item.ID] = true
		if _, _, err := selectedText(u.Text, item); err != nil {
			return err
		}
	}
	if d.Primary != "" && !seen[d.Primary] {
		return fmt.Errorf("the starting point must be included in the excerpt")
	}
	return nil
}

// Browser selection offsets are UTF-16 code units. Reject split surrogate
// pairs so an emoji can never silently become a replacement character.
func selectedText(text string, item Selection) (string, bool, error) {
	if item.Start == nil && item.End == nil {
		return text, false, nil
	}
	if item.Start == nil || item.End == nil {
		return "", false, fmt.Errorf("a passage needs both a start and an end")
	}
	u := utf16.Encode([]rune(text))
	start, end := *item.Start, *item.End
	boundary := func(i int) bool { return i == 0 || i == len(u) || !(u[i] >= 0xDC00 && u[i] <= 0xDFFF) }
	if start < 0 || end <= start || end > len(u) || !boundary(start) || !boundary(end) {
		return "", false, fmt.Errorf("the selected passage has invalid text boundaries")
	}
	return string(utf16.Decode(u[start:end])), start != 0 || end != len(u), nil
}

// Reference-style Markdown needs its definitions. Never silently pull a
// definition from omitted text: the sender must include it explicitly.
var refLink = regexp.MustCompile(`!?\[([^\]\n]+)\]\[([^\]\n]*)\]|\[\^([^\]\n]+)\]`)
var refDefinition = regexp.MustCompile(`(?m)^ {0,3}\[([^\]\n]+)\]:[ \t]*\S`)
var inlineCode = regexp.MustCompile("`+[^`\\n]*`+")
var shortcutRef = regexp.MustCompile(`\[([^\]\n]+)\]`)

// Keep original definition bytes, including inline code in footnotes. Code
// examples are not source references and must not become selectable citations.
func definitionBlocks(text string) []string {
	var blocks []string
	lines := strings.Split(text, "\n")
	fence := ""
	for i := 0; i < len(lines); i++ {
		line := lines[i]
		trim := strings.TrimSpace(line)
		if strings.HasPrefix(trim, "```") || strings.HasPrefix(trim, "~~~") {
			mark := trim[:3]
			if fence == "" {
				fence = mark
			} else if fence == mark {
				fence = ""
			}
			continue
		}
		if fence != "" || !refDefinition.MatchString(line) {
			continue
		}
		start := i
		for i+1 < len(lines) && strings.TrimSpace(lines[i+1]) != "" && (strings.HasPrefix(lines[i+1], "  ") || strings.HasPrefix(lines[i+1], "\t")) && !refDefinition.MatchString(lines[i+1]) {
			i++
		}
		blocks = append(blocks, strings.Join(lines[start:i+1], "\n"))
	}
	return blocks
}

func proseOnly(text string) string {
	var out strings.Builder
	fence := ""
	for _, line := range strings.Split(text, "\n") {
		trim := strings.TrimSpace(line)
		if strings.HasPrefix(trim, "```") || strings.HasPrefix(trim, "~~~") {
			mark := trim[:3]
			if fence == "" {
				fence = mark
			} else if fence == mark {
				fence = ""
			}
			continue
		}
		if fence == "" {
			out.WriteString(inlineCode.ReplaceAllString(line, ""))
			out.WriteByte('\n')
		}
	}
	return out.String()
}

func definitions(texts []string) map[string]bool {
	defs := map[string]bool{}
	for _, text := range texts {
		for _, block := range definitionBlocks(text) {
			m := refDefinition.FindStringSubmatch(block)
			defs[strings.ToLower(strings.TrimSpace(m[1]))] = true
		}
	}
	return defs
}

func missingReferenceLabels(texts []string, known map[string]bool) map[string]bool {
	defs, missing := definitions(texts), map[string]bool{}
	for _, raw := range texts {
		text := proseOnly(raw)
		for _, m := range refLink.FindAllStringSubmatch(text, -1) {
			label := m[2]
			if label == "" {
				label = m[1]
			}
			if m[3] != "" {
				label = "^" + m[3]
			}
			label = strings.ToLower(strings.TrimSpace(label))
			if !defs[label] {
				missing[label] = true
			}
		}
		for _, m := range shortcutRef.FindAllStringSubmatchIndex(text, -1) {
			if m[1] < len(text) && (text[m[1]] == '(' || text[m[1]] == ':' || text[m[1]] == '[') {
				continue
			}
			label := strings.ToLower(strings.TrimSpace(text[m[2]:m[3]]))
			if known[label] && !defs[label] {
				missing[label] = true
			}
		}
	}
	return missing
}

func missingReferences(texts []string) bool {
	return len(missingReferenceLabels(texts, nil)) > 0
}

// Export constructs a new document using a positive allowlist. No original
// raw payload, history, metadata, model parameters, attachment manifest,
// source path, message extras, or arbitrary extension can ride along.
func Export(run map[string]any, cat Catalog, draft Draft) (map[string]any, error) {
	if err := draft.Validate(cat, true); err != nil {
		return nil, err
	}
	items := map[string]Selection{}
	units := map[string]Unit{}
	for _, unit := range cat.Units {
		units[unit.ID] = unit
	}
	for _, item := range draft.Items {
		items[item.ID] = item
	}
	spans := []any{map[string]any{"id": "root", "type": "agent", "name": "Selected session material"}}
	envelope := map[string]any{"kind": "excerpt", "omissions": true}
	if fidelity := str(object(run["source"])["fidelity"]); fidelity == "exact" || fidelity == "reconstructed" || fidelity == "partial" {
		envelope["original_fidelity"] = fidelity
	}
	if draft.Note != "" {
		spans = append(spans, map[string]any{"id": "author-note", "parent_id": "root", "type": "custom", "name": "Author context",
			"input": map[string]any{"messages": []any{map[string]any{"role": "author", "content": []any{map[string]any{"type": "text", "text": draft.Note}}}}}})
		envelope["note_id"] = "author-note"
	}
	var records []any
	texts := []string{draft.Note}
	var sourceTexts []string
	for _, unit := range cat.Units {
		if unit.Kind == "text" || unit.Kind == "source_reference" || unit.Kind == "thinking" {
			sourceTexts = append(sourceTexts, unit.Text)
		}
	}
	last := -1
	for pos, unit := range cat.Units {
		item, selected := items[unit.ID]
		if !selected {
			continue
		}
		text, clipped, _ := selectedText(unit.Text, item)
		if unit.Kind == "text" || unit.Kind == "source_reference" || unit.Kind == "thinking" {
			texts = append(texts, text)
		}
		id := unit.ID
		if item.Start != nil {
			id += fmt.Sprintf("-%d-%d", *item.Start, *item.End)
		}
		label := unit.Role
		switch unit.Kind {
		case "tool_call":
			label = "Tool arguments"
		case "tool_result":
			label = "Tool result"
		case "error":
			label = "Recorded error"
		case "thinking":
			label = "Recorded reasoning"
		case "data":
			label = "Recorded data"
		case "source_reference":
			label = "Source reference"
		}
		span := map[string]any{"id": id, "parent_id": "root", "type": "custom", "name": label,
			"input": map[string]any{"messages": []any{map[string]any{"role": unit.Role, "content": []any{map[string]any{"type": "text", "text": text}}}}}}
		spans = append(spans, span)
		gap := false
		for i := last + 1; i < pos; i++ {
			if cat.Units[i].Kind != "source_reference" {
				gap = true
				break
			}
		}
		record := map[string]any{"id": id, "kind": unit.Kind, "role": unit.Role, "passage": clipped, "omitted_before": gap}
		if unit.Kind == "error" || unit.Role == "assistant" || unit.Role == "tool" {
			present := len(unit.PromptIDs) > 0 && !unit.PromptIncomplete
			for _, promptID := range unit.PromptIDs {
				sel, ok := items[promptID]
				_, clipped, _ := selectedText(units[promptID].Text, sel)
				if !ok || clipped {
					present = false
				}
			}
			if !present {
				record["prompt_missing"] = true
			}
		}
		if unit.Kind == "tool_result" {
			found := false
			for _, other := range cat.Units {
				if _, ok := items[other.ID]; ok && unit.CallID != "" && other.Scope == unit.Scope && other.CallID == unit.CallID && other.Kind == "tool_call" {
					found = true
					break
				}
			}
			if !found {
				record["tool_call_missing"] = true
			}
		}
		records = append(records, record)
		last = pos
		if unit.ID == draft.Primary {
			envelope["primary_id"] = id
		}
	}
	known := definitions(sourceTexts)
	for label := range missingReferenceLabels(texts, known) {
		if known[label] {
			return nil, fmt.Errorf("a selected passage uses a source reference whose definition is omitted; include its Source reference card before previewing")
		}
		envelope["references_unavailable"] = true
	}
	envelope["items"] = records
	if envelope["primary_id"] == nil {
		envelope["primary_id"] = records[0].(map[string]any)["id"]
	}
	out := map[string]any{"schema": "session/v0", "created_at": run["created_at"], "name": draft.Title,
		"source":     map[string]any{"kind": "import", "label": "session.link excerpt", "fidelity": "partial"},
		"extensions": map[string]any{Extension: envelope}, "spans": spans}
	if issues := format.ValidateRun(out); len(issues) != 0 {
		return nil, fmt.Errorf("cannot create excerpt: %s", strings.Join(issues, "; "))
	}
	return out, nil
}
