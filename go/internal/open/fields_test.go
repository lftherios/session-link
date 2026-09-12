package open

import (
	"bytes"
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"

	"github.com/lftherios/session-link/internal/handoff"
	"strings"
	"testing"
)

func TestObjectFieldsSkipsStringsAndNesting(t *testing.T) {
	doc := `{ "a" : "x\"}{,[" , "b":{"c":[1,{"d":"\\"}],"e":null},"name":"n","n":-1.5e3,"t":true,"arr":[ ],"last":"\\\\"}`
	got := objectFields([]byte(doc), "b", "name", "n", "t", "arr", "last", "missing")
	want := map[string]string{"b": `{"c":[1,{"d":"\\"}],"e":null}`, "name": `"n"`, "n": `-1.5e3`, "t": `true`, "arr": `[ ]`, "last": `"\\\\"`}
	if len(got) != len(want) {
		t.Fatalf("got %q", got)
	}
	for key, value := range want {
		if string(got[key]) != value {
			t.Fatalf("%s = %q, want %q", key, got[key], value)
		}
	}
	array := ` [ 1 , "]" , [2,3], {"a":"]\""} ] `
	for text, count := range map[string]int{`[]`: 0, array: 4, `{"a":1}`: 0, ``: 0, `[,]`: 0} {
		if got := countElements([]byte(text)); got != count {
			t.Fatalf("%s has %d elements, counted %d", text, count, got)
		}
	}
	// Truncated or malformed input must neither panic nor loop.
	for _, text := range []string{doc, array, `{"a":,}`, `{"a" "b"}`} {
		for i := range len(text) + 1 {
			objectFields([]byte(text[:i]), "b", "last")
			countElements([]byte(text[:i]))
		}
	}
}

func TestSessionPageScanMatchesFullDecode(t *testing.T) {
	docs := map[string][]byte{
		"unexpected types": []byte(`{"schema":"session/v0","name":7,"spans":{},"metadata":"x","source":{"harness":["cc"]},"extensions":{"session_link.share.v1":null}}`),
		"repeated keys":    []byte("{\n  \"schema\": \"run/v0\",\n  \"name\": \"first\", \"name\": \"second <b>\",\n  \"spans\": [{\"id\": \"a\"}, {\"id\": \"]\\\"\"}],\n  \"metadata\": {\"in_progress\": true, \"session_id\": \"s1\"},\n  \"source\": {\"harness\": \"codex\"}\n}"),
	}
	files, _ := filepath.Glob("../../../testdata/import/*/*/golden.run.json")
	files = append(files, "../../../testdata/share/research/excerpt.json")
	for _, file := range files {
		data, err := os.ReadFile(file)
		if err != nil {
			t.Fatal(err)
		}
		docs[file] = data
	}
	var covered sessionPage
	for name, doc := range docs {
		var compact bytes.Buffer
		if err := json.Compact(&compact, doc); err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		want, err := decodeSessionPage(compact.Bytes())
		if err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if got := scanSessionPage(compact.Bytes()); !reflect.DeepEqual(got, want) {
			t.Fatalf("%s: scanned %+v, decoded %+v", name, got, want)
		}
		indented := scanSessionPage(doc)
		indented.json = want.json
		if !reflect.DeepEqual(indented, want) {
			t.Fatalf("%s: scanned %+v from the original bytes, decoded %+v", name, indented, want)
		}
		covered.spans += want.spans
		covered.inProgress = covered.inProgress || want.inProgress
		covered.excerpt = covered.excerpt || want.excerpt
		covered.titleKey += want.titleKey
	}
	if covered.spans == 0 || !covered.inProgress || !covered.excerpt || covered.titleKey == "" {
		t.Fatalf("fixtures no longer exercise every field: %+v", covered)
	}
}

func TestSessionPageDecodesPreviewsWhoseBytesDoNotMatchTheirName(t *testing.T) {
	s := &Server{Project: "/work/project", PreviewDir: t.TempDir(), Target: "http://127.0.0.1:1"}
	id := strings.Repeat("a", 64)
	write := func(body string) {
		t.Helper()
		if err := os.WriteFile(filepath.Join(s.PreviewDir, id+".json"), []byte(body), 0o600); err != nil {
			t.Fatal(err)
		}
	}
	write("{\n  \"schema\": \"session/v0\",\n  \"name\": \"Edited by hand\",\n  \"spans\": []\n}")
	page := action(s, "GET", "/p/"+id, "", "")
	if page.Code != 200 || !strings.Contains(page.Body.String(), `{"schema":"session/v0","name":"Edited by hand","spans":[]}`) {
		t.Fatalf("%d %s", page.Code, page.Body)
	}
	write(`{"schema":"other/v1","spans":[]}`)
	if page := action(s, "GET", "/p/"+id, "", ""); page.Code != 500 {
		t.Fatalf("non-session JSON served: %d", page.Code)
	}
}

func TestSessionPageCarriesTheReadingCopyAndServesTheWholeDocument(t *testing.T) {
	raw := previewFixture(t)
	var doc map[string]any
	if err := json.Unmarshal(raw, &doc); err != nil {
		t.Fatal(err)
	}
	marker := "recorded-output-marker"
	for _, value := range doc["spans"].([]any) {
		span, _ := value.(map[string]any)
		if span["type"] != "tool_call" {
			continue
		}
		output, _ := span["output"].(map[string]any)
		if output == nil {
			output = map[string]any{}
			span["output"] = output
		}
		output["result"] = marker + strings.Repeat("x", 900<<10)
		break
	}
	data, err := json.Marshal(doc)
	if err != nil {
		t.Fatal(err)
	}
	dir := t.TempDir()
	id, err := handoff.Save(dir, handoff.Source{Read: func() ([]byte, error) { return data, nil }})
	if err != nil {
		t.Fatal(err)
	}
	s := &Server{Project: "/work/project", PreviewDir: dir, Target: "http://127.0.0.1:1"}
	page := action(s, "GET", "/p/"+id, "", "").Body.String()
	if strings.Contains(page, marker) {
		t.Fatal("the page carried recorded tool output")
	}
	for _, want := range []string{`window.__FULL__="/api/document/` + id + `"`, `"result_omitted":true`} {
		if !strings.Contains(page, want) {
			t.Fatalf("page is missing %s", want)
		}
	}
	whole := action(s, "GET", "/api/document/"+id, "", "")
	if whole.Code != 200 || !strings.Contains(whole.Body.String(), marker) {
		t.Fatalf("the whole document is not served: %d", whole.Code)
	}
	if missing := action(s, "GET", "/api/document/"+strings.Repeat("b", 64), "", ""); missing.Code != 404 {
		t.Fatalf("a session that is not saved answered %d", missing.Code)
	}
}
