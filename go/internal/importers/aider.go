// Aider chat history → session/v0. Aider appends every run to one Markdown
// file per project, .aider.chat.history.md, and marks each line by who it is
// from: "#### " is what the person typed, "> " is aider's own notice (the
// model in use, an edit it applied, a commit, a command's question), and
// anything else is the model's reply. A run starts at a "# aider chat started
// at" line. The file holds no tool calls, since aider's edits are text in the
// reply, and no times after the first, so a reply becomes an llm_call with
// the typed lines before it as its input, and aider's notices are kept where
// they appeared as the evidence of what it did. The notices ahead of the
// first typed line are the run's setup.
//
// The marks are aider's only structure. A reply line that itself begins with
// "> " or "#### " reads as a notice or as typed text, here as in aider's own
// reader.
package importers

import (
	"fmt"
	"regexp"
	"strings"
	"time"
)

func init() { Registry["aider"] = importAider }

var (
	// aiderStart opens a run, in the local time of the machine aider ran on.
	aiderStart = regexp.MustCompile(`^# aider chat started at (\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2})\s*$`)
	// aider announces itself and its model when a run starts, and again when
	// the model is switched.
	aiderVersion = regexp.MustCompile(`^Aider v(\S+)`)
	aiderModel   = regexp.MustCompile(`^(?:Main model|Model): (\S+?),? `)
)

// aiderZone is the zone aider's clock times are read in. Aider writes them
// without one; they are the local time of the machine, which is where a
// session is imported.
var aiderZone = time.Local

const (
	aiderStamp      = "2006-01-02 15:04:05"
	aiderInputStamp = "2006-01-02 15:04:05.999999"
)

// aiderTime reads one of aider's clock times.
func aiderTime(layout, text string) (time.Time, bool) {
	at, err := time.ParseInLocation(layout, text, aiderZone)
	return at, err == nil
}

func aiderISO(at time.Time) string {
	return at.UTC().Format("2006-01-02T15:04:05.000Z")
}

// aiderID names a run inside its history file: the time it started, as aider
// wrote it.
func aiderID(stamp string) string {
	return strings.Replace(stamp, " ", "T", 1)
}

// aiderBlock is a stretch of lines from one writer: 'u' the person, 'a' the
// model, 'n' aider itself.
type aiderBlock struct {
	kind  byte
	lines []string
}

// aiderBlocks splits a run into blocks. Aider puts a blank line before what
// the person typed and around a reply; blank lines inside a reply are part of
// it.
func aiderBlocks(lines []string) []aiderBlock {
	var blocks []aiderBlock
	add := func(kind byte, line string) {
		if n := len(blocks); n > 0 && blocks[n-1].kind == kind {
			blocks[n-1].lines = append(blocks[n-1].lines, line)
			return
		}
		blocks = append(blocks, aiderBlock{kind: kind, lines: []string{line}})
	}
	typing := false // inside one typed entry, which a blank line ends
	for _, line := range lines {
		line = strings.TrimSuffix(line, "\r")
		switch {
		case strings.HasPrefix(line, "#### ") || line == "####":
			text := strings.TrimRight(strings.TrimPrefix(strings.TrimPrefix(line, "####"), " "), " ")
			if typing {
				add('u', text)
			} else {
				blocks = append(blocks, aiderBlock{kind: 'u', lines: []string{text}})
			}
			typing = true
		case strings.HasPrefix(line, "> ") || line == ">":
			add('n', strings.TrimRight(strings.TrimPrefix(strings.TrimPrefix(line, ">"), " "), " "))
			typing = false
		case strings.TrimSpace(line) == "":
			if n := len(blocks); n > 0 && blocks[n-1].kind == 'a' && !typing {
				blocks[n-1].lines = append(blocks[n-1].lines, "")
			}
			typing = false
		default:
			add('a', line)
			typing = false
		}
	}
	return blocks
}

func aiderText(lines []string) string {
	return strings.TrimSpace(strings.Join(lines, "\n"))
}

// importAider reads one run. Lines are that run's lines, blank ones included,
// from its "# aider chat started at" line on. Session carries what the file
// around it says: the project directory, and the entries of aider's input
// history that fall in the run, which give the time each one was typed.
func importAider(in Input) (map[string]any, error) {
	lines := in.Lines
	var started time.Time
	stamp := ""
	if len(lines) > 0 {
		if match := aiderStart.FindStringSubmatch(strings.TrimSuffix(lines[0], "\r")); match != nil {
			stamp = match[1]
			started, _ = aiderTime(aiderStamp, stamp)
			lines = lines[1:]
		}
	}
	if started.IsZero() {
		return nil, fmt.Errorf("not an aider run: it does not begin with a \"# aider chat started at\" line")
	}
	blocks := aiderBlocks(lines)

	root := map[string]any{"id": "root", "parent_id": nil, "type": "agent", "status": "ok", "started_at": aiderISO(started)}
	spans := []any{root}
	seq := 0
	next := func() string {
		seq++
		return fmt.Sprintf("s%d", seq)
	}

	// The times the person's entries were typed, in order, for the entries
	// whose text is in the input history exactly as it is in the chat.
	typed := arr(in.Session["inputs"])
	cursor := 0
	typedAt := func(text string) (time.Time, bool) {
		for i := cursor; i < len(typed); i++ {
			entry := m(typed[i])
			if strings.TrimSpace(strOr(entry["text"], "")) == text {
				if at, ok := aiderTime(aiderInputStamp, strOr(entry["at"], "")); ok {
					cursor = i + 1
					return at, true
				}
			}
		}
		return time.Time{}, false
	}

	version, model, prompt := "", "unknown", ""
	pending := []any{}
	var pendingAt time.Time // when the first waiting entry was typed
	timed := func(span map[string]any) {
		if !pendingAt.IsZero() {
			span["started_at"] = aiderISO(pendingAt)
		}
		pendingAt = time.Time{}
	}
	// flush records typed entries that got no reply: a command to aider
	// itself, or a prompt that was interrupted.
	flush := func() {
		if len(pending) == 0 {
			return
		}
		name := strOr(m(first(arr(m(pending[0])["content"])))["text"], "")
		if cut, _, more := strings.Cut(name, "\n"); more {
			name = cut
		}
		span := map[string]any{"id": next(), "parent_id": "root", "type": "custom", "name": name,
			"input": map[string]any{"messages": pending}}
		timed(span)
		spans = append(spans, span)
		pending = []any{}
	}

	begun := false // something has been typed or answered
	for _, block := range blocks {
		text := aiderText(block.lines)
		if block.kind != 'n' {
			begun = true
		}
		switch block.kind {
		case 'u':
			if at, ok := typedAt(text); ok && len(pending) == 0 {
				pendingAt = at
			}
			if prompt == "" && text != "" && !strings.HasPrefix(text, "/") {
				prompt = text
			}
			pending = append(pending, map[string]any{"role": "user", "content": []any{map[string]any{"type": "text", "text": text}}})
		case 'a':
			if text == "" {
				continue
			}
			span := map[string]any{
				"id": next(), "parent_id": "root", "type": "llm_call", "name": "turn", "status": "ok",
				"model": map[string]any{"id": model},
				"input": map[string]any{"messages": pending},
				"output": map[string]any{"messages": []any{map[string]any{
					"role": "assistant", "content": []any{map[string]any{"type": "text", "text": text}},
				}}},
			}
			timed(span)
			spans = append(spans, span)
			pending = []any{}
		case 'n':
			flush()
			for _, line := range block.lines {
				if match := aiderVersion.FindStringSubmatch(line); match != nil {
					version = match[1]
				}
				if match := aiderModel.FindStringSubmatch(line + " "); match != nil {
					model = match[1]
				}
			}
			if !begun {
				// What aider says before anything is typed is the setup of
				// the run: its version, the model, the files in the chat.
				spans = append(spans, map[string]any{
					"id": next(), "parent_id": "root", "type": "custom", "name": "aider startup",
					"input": map[string]any{"messages": []any{map[string]any{
						"role": "system", "content": []any{map[string]any{"type": "text", "text": strings.Join(block.lines, "\n")}},
					}}},
				})
				continue
			}
			// What aider reported doing. It runs its own tools (applying
			// edits, committing, linting) and this is all it says of them.
			spans = append(spans, map[string]any{
				"id": next(), "parent_id": "root", "type": "tool_call", "name": "aider",
				"input":  map[string]any{"name": "aider"},
				"output": map[string]any{"result": strings.Join(block.lines, "\n")},
			})
		}
	}
	flush()

	name := prompt
	if cut, _, more := strings.Cut(name, "\n"); more {
		name = cut
	}
	if cut := []rune(name); len(cut) > 80 {
		name = string(cut[:80]) + "…"
	}
	if name == "" {
		name = "aider " + stamp
	}
	root["name"] = name

	metadata := map[string]any{"session_id": aiderID(stamp)}
	if cwd, ok := str(in.Session["cwd"]); ok && cwd != "" {
		metadata["cwd"] = cwd
	}
	if version != "" {
		metadata["harness_version"] = version
	}
	return map[string]any{
		"schema":     "session/v0",
		"name":       name,
		"created_at": aiderISO(started),
		"source": map[string]any{
			"kind": "import", "harness": "aider",
			"label": "session-import@0.1.0", "fidelity": "reconstructed",
		},
		"metadata": metadata,
		"spans":    spans,
	}, nil
}
