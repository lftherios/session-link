package importers

import (
	"encoding/json"
	"strings"
	"testing"

	"github.com/lftherios/session-link/internal/format"
)

func TestCodexReasoningAvailability(t *testing.T) {
	for _, tc := range []struct {
		name         string
		payload      map[string]any
		text, reason string
	}{
		{"summary", map[string]any{"summary": []any{map[string]any{"text": "Readable summary"}}, "encrypted_content": "CIPHERTEXT_MUST_NOT_ENTER_TEXT"}, "Readable summary", ""},
		{"content after empty summary", map[string]any{"summary": []any{}, "content": []any{map[string]any{"text": "Readable content"}}}, "Readable content", ""},
		{"encrypted only", map[string]any{"summary": []any{}, "encrypted_content": "CIPHERTEXT_MUST_NOT_ENTER_TEXT"}, "", "encrypted"},
		{"no recorded text", map[string]any{"summary": []any{}}, "", "not_recorded"},
	} {
		t.Run(tc.name, func(t *testing.T) {
			part := codexReasoningPart(tc.payload)
			if part["text"] != tc.text {
				t.Fatalf("reasoning text: %+v", part)
			}
			if tc.reason == "" {
				if part["unavailable"] == true {
					t.Fatal("readable reasoning marked unavailable")
				}
			} else if part["unavailable"] != true || part["reason"] != tc.reason {
				t.Fatalf("missing availability marker: %+v", part)
			}
			raw, _ := json.Marshal(part)
			if strings.Contains(string(raw), "CIPHERTEXT_MUST_NOT_ENTER_TEXT") || strings.Contains(string(raw), "[reasoning]") {
				t.Fatal("ciphertext or a fabricated placeholder entered readable content")
			}
		})
	}
}

func TestCodexKeepsAnUnavailableReasoningEventWithoutAnAnswer(t *testing.T) {
	lines := []map[string]any{
		{"timestamp": "2026-09-11T00:00:00Z", "type": "response_item", "payload": map[string]any{"type": "message", "role": "user", "content": []any{map[string]any{"type": "input_text", "text": "An interrupted task"}}}},
		{"timestamp": "2026-09-11T00:00:01Z", "type": "response_item", "payload": map[string]any{"type": "reasoning", "summary": []any{}, "encrypted_content": "CIPHERTEXT_MUST_NOT_ENTER_TEXT"}},
	}
	var encoded []string
	for _, line := range lines {
		raw, err := json.Marshal(line)
		if err != nil {
			t.Fatal(err)
		}
		encoded = append(encoded, string(raw))
	}
	run := codexRolloutToRun(encoded, "Reasoning availability")
	if issues := format.ValidateRun(run); len(issues) != 0 {
		t.Fatal(issues)
	}
	raw, _ := json.Marshal(run)
	if !strings.Contains(string(raw), `"unavailable":true`) || strings.Contains(string(raw), "CIPHERTEXT_MUST_NOT_ENTER_TEXT") || strings.Contains(string(raw), "[reasoning]") {
		t.Fatal("unavailable reasoning event lost or represented as text")
	}
}
