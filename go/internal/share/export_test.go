package share

import (
	"encoding/json"
	"os"
	"strings"
	"testing"
)

func fixture(t *testing.T, file string, dst any) {
	t.Helper()
	data, err := os.ReadFile("../../../testdata/share/research/" + file)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(data, dst); err != nil {
		t.Fatal(err)
	}
}
func ptr(i int) *int { return &i }
func containsUnit(cat Catalog, text string) []Unit {
	var out []Unit
	for _, u := range cat.Units {
		if strings.Contains(u.Text, text) {
			out = append(out, u)
		}
	}
	return out
}

func TestResearchExportOmitsEveryUnselectedChannel(t *testing.T) {
	var run map[string]any
	var draft Draft
	fixture(t, "session.json", &run)
	fixture(t, "draft.json", &draft)
	original := canonical(run)
	cat := BuildCatalog(run)
	if len(containsUnit(cat, "Compare Atlas")) != 1 {
		t.Fatal("replayed prompt duplicated")
	}
	out, err := Export(run, cat, draft)
	if err != nil {
		t.Fatal(err)
	}
	bytes := canonical(out)
	var expected map[string]any
	fixture(t, "excerpt.json", &expected)
	if canonical(expected) != bytes {
		t.Fatalf("research export differs from the independently specified example:\n%s", bytes)
	}
	for _, excluded := range []string{"OMITTED_INTERNAL_STRATEGY", "A comparison of the two fictional", "Internal planning", "private_extra", "attachments", "raw", "params", "input_tokens"} {
		if strings.Contains(bytes, excluded) {
			t.Fatalf("excluded content survived in export: %s", excluded)
		}
	}
	for _, included := range []string{"Compare Atlas and Beacon", "Atlas documents a self-serve", "🔎", "https://atlas.example.test/onboarding", draft.Note} {
		if !strings.Contains(bytes, included) {
			t.Fatalf("selected content missing: %s", included)
		}
	}
	if canonical(run) != original {
		t.Fatal("source mutated by export")
	}
	ext := object(object(out["extensions"])[Extension])
	if ext["original_fidelity"] != "exact" || object(out["source"])["fidelity"] != "partial" {
		t.Fatal("source and excerpt fidelity conflated")
	}
	items := array(ext["items"])
	if object(items[0])["role"] != "user" || object(items[1])["passage"] != true || object(items[1])["prompt_missing"] == true {
		t.Fatalf("context or chronology lost: %v", items)
	}
	if str(ext["primary_id"]) == str(object(items[0])["id"]) {
		t.Fatal("starting point replaced by earlier prompt")
	}
	// Reordering selection requests must not change the exported document.
	draft.Items[0], draft.Items[1] = draft.Items[1], draft.Items[0]
	again, err := Export(run, cat, draft)
	if err != nil || canonical(again) != bytes {
		t.Fatal("request order changed export")
	}
}

func TestSelectionsAreVerbatimAndValidateUTF16Boundaries(t *testing.T) {
	text := "before 🔎 café after"
	got, clipped, err := selectedText(text, Selection{Start: ptr(7), End: ptr(14)})
	if err != nil || got != "🔎 café" || !clipped {
		t.Fatalf("%q %v %v", got, clipped, err)
	}
	for _, item := range []Selection{
		{Start: ptr(8), End: ptr(14)}, {Start: ptr(7), End: ptr(8)}, {Start: ptr(-1), End: ptr(4)},
		{Start: ptr(0), End: ptr(999)}, {Start: ptr(4), End: ptr(3)}, {Start: ptr(0)},
	} {
		if _, _, err := selectedText(text, item); err == nil {
			t.Fatalf("accepted invalid passage %+v", item)
		}
	}
}

func TestDraftRejectsUnknownDuplicateAndUnavailableSelections(t *testing.T) {
	cat := Catalog{Units: []Unit{{ID: "text", Text: "a"}, {ID: "blob", Unavailable: true}}}
	for _, draft := range []Draft{
		{Title: "a"}, {Title: "a", Items: []Selection{{ID: "unknown"}}}, {Title: "a", Items: []Selection{{ID: "blob"}}},
		{Title: "a", Items: []Selection{{ID: "text"}, {ID: "text"}}}, {Title: "a", Items: []Selection{{ID: "text"}}, Primary: "absent"},
		{Title: strings.Repeat("x", 257), Items: []Selection{{ID: "text"}}},
	} {
		if err := draft.Validate(cat, true); err == nil {
			t.Fatalf("accepted %+v", draft)
		}
	}
	if err := (Draft{}).Validate(cat, false); err != nil {
		t.Fatal("empty drafts must be saveable")
	}
}

func TestMissingPromptAndToolContextAreExplicit(t *testing.T) {
	var run map[string]any
	raw, err := os.ReadFile("../../../testdata/import/pi/error-and-trailing/golden.run.json")
	if err != nil {
		t.Fatal(err)
	}
	json.Unmarshal(raw, &run)
	cat := BuildCatalog(run)
	var result, call *Unit
	for i := range cat.Units {
		u := &cat.Units[i]
		if u.Kind == "tool_result" {
			result = u
		}
		if u.Kind == "tool_call" {
			call = u
		}
	}
	if result == nil || call == nil {
		t.Fatalf("tool evidence missing: %+v", cat)
	}
	draft := Draft{Title: "Why did this fail?", Items: []Selection{{ID: result.ID}}, Primary: result.ID}
	out, err := Export(run, cat, draft)
	if err != nil {
		t.Fatal(err)
	}
	item := object(array(object(object(out["extensions"])[Extension])["items"])[0])
	if item["prompt_missing"] != true || item["tool_call_missing"] != true {
		t.Fatal(item)
	}
	draft.Items = append(draft.Items, Selection{ID: call.ID})
	for _, id := range result.PromptIDs {
		draft.Items = append(draft.Items, Selection{ID: id})
	}
	out, err = Export(run, cat, draft)
	if err != nil {
		t.Fatal(err)
	}
	for _, record := range array(object(object(out["extensions"])[Extension])["items"]) {
		item := object(record)
		if item["kind"] == "tool_result" && (item["prompt_missing"] == true || item["tool_call_missing"] == true) {
			t.Fatal(item)
		}
	}
}

func TestReferencesMustTravelWithTheirDefinitions(t *testing.T) {
	if !missingReferences([]string{"See [the docs][onboarding]."}) {
		t.Fatal("dangling reference accepted")
	}
	if missingReferences([]string{"See [the docs][onboarding].", "[onboarding]: https://example.test/docs"}) {
		t.Fatal("included definition rejected")
	}
	if missingReferences([]string{"Read [docs](https://example.test/docs)."}) {
		t.Fatal("inline URL rejected")
	}
	if missingReferences([]string{"Use `matrix[i][j]`.\n```python\nx = matrix[i][j]\n```"}) {
		t.Fatal("code was mistaken for a citation")
	}
}
