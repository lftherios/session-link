package normalize

import (
	"encoding/json"
	"os"
	"path/filepath"
	"reflect"
	"testing"
)

// TestGoldenProxyCases replicates scripts/golden.mjs's proxy pipeline over
// the same fixtures: parity is parsed equality of the built span.
func TestGoldenProxyCases(t *testing.T) {
	wd, err := os.Getwd()
	if err != nil {
		t.Fatal(err)
	}
	casesDir := filepath.Join(wd, "..", "..", "..", "testdata", "proxy")
	entries, err := os.ReadDir(casesDir)
	if err != nil {
		t.Fatalf("no proxy fixtures: %v", err)
	}
	ran := 0
	for _, e := range entries {
		if !e.IsDir() {
			continue
		}
		ran++
		t.Run(e.Name(), func(t *testing.T) {
			dir := filepath.Join(casesDir, e.Name())
			var meta struct {
				Kind           string `json:"kind"`
				Streamed       bool   `json:"streamed"`
				StartedAt      string `json:"started_at"`
				EndedAt        string `json:"ended_at"`
				HTTPStatus     int    `json:"httpStatus"`
				TransportError string `json:"transportError"`
			}
			readJSON(t, filepath.Join(dir, "input.meta.json"), &meta)
			var request any
			readJSON(t, filepath.Join(dir, "input.request.json"), &request)

			var response any
			captureGap := false
			if meta.Streamed {
				sseBytes, err := os.ReadFile(filepath.Join(dir, "input.response.sse"))
				if err != nil {
					t.Fatal(err)
				}
				sse := string(sseBytes)
				assembled := assembleByKind(meta.Kind, ParseSseText(sse))
				if assembled == nil {
					captureGap = true
					// The harness slices at 100k JS chars; fixtures are ASCII.
					cut := sse
					if len(cut) > 100_000 {
						cut = cut[:100_000]
					}
					response = map[string]any{"unassembled_sse": cut}
				} else {
					response = assembled
				}
			} else {
				readJSON(t, filepath.Join(dir, "input.response.json"), &response)
			}

			span := BuildLlmSpan(meta.Kind, BuildContext{
				ID: "s1", ParentID: "root",
				Request: request, Response: response,
				StartedAt: meta.StartedAt, EndedAt: meta.EndedAt,
				HTTPStatus: meta.HTTPStatus, Streamed: meta.Streamed,
				CaptureGap: captureGap, TransportError: meta.TransportError,
			})

			var golden any
			readJSON(t, filepath.Join(dir, "golden.span.json"), &golden)
			got := roundTrip(t, span)
			if !reflect.DeepEqual(got, golden) {
				g, _ := json.MarshalIndent(got, "", "  ")
				w, _ := json.MarshalIndent(golden, "", "  ")
				t.Fatalf("span mismatch\n--- got ---\n%s\n--- want ---\n%s", g, w)
			}
		})
	}
	if ran == 0 {
		t.Fatal("zero proxy fixtures ran — the parity test passed vacuously")
	}
}

func assembleByKind(kind string, events []any) map[string]any {
	switch kind {
	case "anthropic.messages":
		return AssembleAnthropicSse(events)
	case "openai.chat":
		return AssembleOpenaiChatSse(events)
	case "openai.responses":
		return AssembleResponsesSse(events)
	}
	return nil
}

func readJSON(t *testing.T, path string, v any) {
	t.Helper()
	b, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := json.Unmarshal(b, v); err != nil {
		t.Fatalf("%s: %v", path, err)
	}
}

// roundTrip normalizes Go-native values (bool, int, nested maps) into the
// same shapes json.Unmarshal produces, so DeepEqual compares JSON values.
func roundTrip(t *testing.T, v any) any {
	t.Helper()
	b, err := json.Marshal(v)
	if err != nil {
		t.Fatal(err)
	}
	var out any
	if err := json.Unmarshal(b, &out); err != nil {
		t.Fatal(err)
	}
	return out
}
