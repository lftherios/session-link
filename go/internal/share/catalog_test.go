package share

import (
	"encoding/json"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func msg(r, text string) any {
	return map[string]any{"role": r, "content": []any{map[string]any{"type": "text", "text": text}}}
}
func call(id, parent string, in, out []any) any {
	return map[string]any{"id": id, "parent_id": parent, "type": "llm_call", "input": map[string]any{"messages": in}, "output": map[string]any{"messages": out}}
}

func TestCatalogKeepsAgentPromptsAndRepeatedDeltaTurns(t *testing.T) {
	q, a := msg("user", "parent prompt"), msg("assistant", "parent answer")
	run := map[string]any{"source": map[string]any{"kind": "proxy"}, "spans": []any{
		map[string]any{"id": "root", "type": "agent"}, call("one", "root", []any{q}, []any{a}),
		map[string]any{"id": "child", "parent_id": "one", "type": "agent"}, call("two", "child", []any{msg("user", "child prompt")}, []any{msg("assistant", "child answer")}),
		call("three", "root", []any{q, a, msg("user", "parent followup")}, []any{msg("assistant", "continued answer")}),
	}}
	cat := BuildCatalog(run)
	if len(containsUnit(cat, "parent prompt")) != 1 {
		t.Fatal("replayed parent prompt duplicated after subagent")
	}
	for answer, prompt := range map[string]string{"child answer": "child prompt", "continued answer": "parent followup"} {
		u := containsUnit(cat, answer)[0]
		p := containsUnit(cat, prompt)[0]
		if len(u.PromptIDs) != 1 || u.PromptIDs[0] != p.ID {
			t.Fatalf("wrong prompt for %s: %v", answer, u.PromptIDs)
		}
	}
	for _, source := range []map[string]any{{"kind": "import"}, {"kind": "sdk", "label": "pi-extension@0.1.0"}} {
		run["source"] = source
		run["spans"] = []any{call("one", "", []any{q}, nil), call("two", "", []any{q}, []any{a})}
		if len(containsUnit(BuildCatalog(run), "parent prompt")) != 2 {
			t.Fatal("repeated delta disappeared")
		}
	}
}

func TestCatalogToolResultsPreserveRepeatedParts(t *testing.T) {
	parts := []any{map[string]any{"type": "text", "text": "same result"}, map[string]any{"type": "text", "text": "same result"}}
	run := map[string]any{"spans": []any{
		call("one", "", []any{msg("user", "run it")}, []any{map[string]any{"role": "assistant", "content": []any{map[string]any{"type": "tool_call", "id": "c1", "name": "bash", "arguments": map[string]any{"command": "echo hi"}}}}}),
		map[string]any{"id": "tool", "type": "tool_call", "parent_id": "one", "name": "bash", "input": map[string]any{"tool_call_id": "c1", "arguments": map[string]any{"command": "echo hi"}}, "output": map[string]any{"result": parts}},
		call("two", "", []any{map[string]any{"role": "tool", "content": []any{map[string]any{"type": "tool_result", "tool_call_id": "c1", "content": parts}}}}, []any{msg("assistant", "done")}),
	}}
	cat := BuildCatalog(run)
	if len(containsUnit(cat, "same result")) != 2 {
		t.Fatal("repeated parts were collapsed or a replay was duplicated")
	}
	if len(containsUnit(cat, "echo hi")) != 1 {
		t.Fatal("message and standalone tool call duplicated")
	}
	if !strings.Contains(containsUnit(cat, "echo hi")[0].Text, "bash") {
		t.Fatal("tool identity missing from selected evidence")
	}
}

func TestPromptWithUnavailablePartsDoesNotPretendToBeComplete(t *testing.T) {
	run := map[string]any{"spans": []any{call("one", "", []any{map[string]any{"role": "user", "content": []any{
		map[string]any{"type": "text", "text": "Explain the picture"}, map[string]any{"type": "image", "url": "https://example.test/private.png"},
	}}}, []any{msg("assistant", "It depicts a forest.")})}}
	cat := BuildCatalog(run)
	answer := containsUnit(cat, "It depicts")[0]
	if !answer.PromptIncomplete || len(answer.PromptIDs) != 1 {
		t.Fatalf("%+v", answer)
	}
	if !cat.Units[1].Unavailable {
		t.Fatal("attachment was offered as shareable text")
	}
}

func TestReferenceDefinitionCanBeSelectedSeparatelyFromPrivateText(t *testing.T) {
	text := "Finding from [docs][source].\n\nDO_NOT_SHARE\n\n[source]: https://example.test/docs"
	run := map[string]any{"created_at": "2026-09-11T00:00:00Z", "spans": []any{call("one", "", nil, []any{msg("assistant", text)})}}
	cat := BuildCatalog(run)
	if len(cat.Units) != 2 || cat.Units[1].Kind != "source_reference" {
		t.Fatal(cat)
	}
	draft := Draft{Title: "Finding", Primary: cat.Units[0].ID, Items: []Selection{{ID: cat.Units[0].ID, Start: ptr(0), End: ptr(28)}}}
	if _, err := Export(run, cat, draft); err == nil {
		t.Fatal("available citation target silently omitted")
	}
	draft.Items = append(draft.Items, Selection{ID: cat.Units[1].ID})
	out, err := Export(run, cat, draft)
	if err != nil {
		t.Fatal(err)
	}
	if strings.Contains(canonical(out), "DO_NOT_SHARE") || !strings.Contains(canonical(out), "https://example.test/docs") {
		t.Fatal("reference target or exclusion lost")
	}
}

func TestAllHarnessFixturesHaveSelectableEvidence(t *testing.T) {
	for _, harness := range []string{"claude-code", "codex", "pi", "opencode", "hermes"} {
		t.Run(harness, func(t *testing.T) {
			raw, err := os.ReadFile(filepath.Join("../../../testdata/import", harness, "basic/golden.run.json"))
			if err != nil {
				t.Fatal(err)
			}
			var run map[string]any
			json.Unmarshal(raw, &run)
			cat := BuildCatalog(run)
			roles := map[string]bool{}
			for _, u := range cat.Units {
				if u.Unavailable {
					continue
				}
				roles[u.Role] = true
				_, err := Export(run, cat, Draft{Title: "Selected evidence", Items: []Selection{{ID: u.ID}}, Primary: u.ID})
				if err != nil {
					t.Fatalf("unit %s: %v", u.ID, err)
				}
			}
			if !roles["user"] || !roles["assistant"] || !roles["tool"] {
				t.Fatalf("missing prompt, output or tool result: %v", roles)
			}
		})
	}
}

func TestReferenceCardsPreserveOriginalTextAndIgnoreCode(t *testing.T) {
	note := "[^source]: `Atlas` documentation\n    Additional detail."
	text := "```md\n[example]: https://example.test/not-a-citation\n```\n\n" + note + "\n[docs]: https://example.test/docs"
	run := map[string]any{"spans": []any{call("one", "", nil, []any{msg("assistant", text)})}}
	cat := BuildCatalog(run)
	if len(cat.Units) != 3 || cat.Units[1].Text != note || cat.Units[2].Text != "[docs]: https://example.test/docs" {
		t.Fatalf("reference cards changed the source or included a code example: %+v", cat.Units)
	}
	if missingReferences([]string{"Finding[^source]", note}) {
		t.Fatal("inline code in a footnote made its definition disappear")
	}
}

func TestUnavailableReasoningCannotBecomeSharedText(t *testing.T) {
	for _, legacy := range []bool{false, true} {
		unavailable := map[string]any{"type": "thinking", "text": "", "unavailable": true, "reason": "encrypted"}
		if legacy {
			unavailable = map[string]any{"type": "thinking", "text": "[reasoning]"}
		}
		run := map[string]any{"schema": "session/v0", "created_at": "2026-09-11T00:00:00Z", "source": map[string]any{"kind": "import", "harness": "codex"}, "spans": []any{
			call("one", "", []any{msg("user", "Compare options")}, []any{map[string]any{"role": "assistant", "content": []any{
				unavailable,
				map[string]any{"type": "thinking", "text": "Actual readable summary"},
				map[string]any{"type": "text", "text": "The answer"},
			}}}),
		}}
		cat := BuildCatalog(run)
		missing, readable, answer := cat.Units[1], cat.Units[2], cat.Units[3]
		if !missing.Unavailable || readable.Unavailable || answer.ID != "u0-out-0-2" {
			t.Fatalf("availability or source addresses lost: %+v", cat)
		}
		if _, err := Export(run, cat, Draft{Title: "Missing", Items: []Selection{{ID: missing.ID}}}); err == nil {
			t.Fatal("unavailable reasoning was exported as real text")
		}
		if _, err := Export(run, cat, Draft{Title: "Readable", Items: []Selection{{ID: readable.ID}, {ID: answer.ID}}}); err != nil {
			t.Fatal(err)
		}
		if legacy {
			run["source"] = map[string]any{"kind": "import", "harness": "another-harness"}
			if BuildCatalog(run).Units[1].Unavailable {
				t.Fatal("the Codex compatibility rule changed another harness's literal text")
			}
		}
	}
}
