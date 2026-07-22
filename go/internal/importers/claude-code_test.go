package importers

import (
	"strings"
	"testing"
)

func ccUser(text string, meta bool) map[string]any {
	e := map[string]any{
		"type":    "user",
		"message": map[string]any{"role": "user", "content": text},
	}
	if meta {
		e["isMeta"] = true
	}
	return e
}

func TestCCFirstUserText(t *testing.T) {
	long := strings.Repeat("word ", 60) // 300 chars once collapsed

	t.Run("clips at 200 runes as a safety net", func(t *testing.T) {
		got := ccFirstUserText([]map[string]any{ccUser(long, false)})
		if r := []rune(got); len(r) != 200 || !strings.HasSuffix(got, "…") {
			t.Fatalf("got %d runes, suffix %q", len(r), got[len(got)-3:])
		}
	})

	t.Run("keeps a real title whole", func(t *testing.T) {
		title := "I want us to focus on UX. I want to have the best possible experience a user can have with the CLI and viewer (before they publish)."
		if got := ccFirstUserText([]map[string]any{ccUser(title, false)}); got != title {
			t.Fatalf("clipped a %d-char title: %q", len(title), got)
		}
	})

	t.Run("skips isMeta, caveats, and tool-shaped rows", func(t *testing.T) {
		entries := []map[string]any{
			ccUser("injected context", true),
			ccUser("Caveat: the messages below were generated…", false),
			ccUser("<tool_result>…</tool_result>", false),
			ccUser("the actual ask", false),
		}
		if got := ccFirstUserText(entries); got != "the actual ask" {
			t.Fatalf("got %q", got)
		}
	})
}
