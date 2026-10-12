// Command verdicts is the Go side of scripts/format-diff.mjs. Each line on
// stdin is a JSON string holding one document's text; each line on stdout is
// what the CLI's publish gate makes of it: the validator's verdict and what
// the credential scan finds.
package main

import (
	"bufio"
	"encoding/json"
	"fmt"
	"io"
	"os"

	"github.com/lftherios/session-link/internal/format"
	"github.com/lftherios/session-link/internal/scan"
)

type verdict struct {
	Verdict string     `json:"verdict"`
	Errors  []string   `json:"errors"`
	Secrets []scan.Hit `json:"secrets"`
}

func main() {
	in := bufio.NewReader(os.Stdin)
	out := bufio.NewWriter(os.Stdout)
	defer out.Flush()
	enc := json.NewEncoder(out)
	enc.SetEscapeHTML(false)
	for {
		line, err := in.ReadBytes('\n')
		if len(line) > 0 {
			var text string
			if err := json.Unmarshal(line, &text); err != nil {
				fmt.Fprintln(os.Stderr, "verdicts: a line is not a JSON string:", err)
				os.Exit(2)
			}
			enc.Encode(judge(text))
		}
		if err == io.EOF {
			return
		}
		if err != nil {
			fmt.Fprintln(os.Stderr, "verdicts:", err)
			os.Exit(2)
		}
	}
}

// judge parses as cli.InspectRunFile does, then validates and scans.
func judge(text string) verdict {
	v := verdict{Secrets: scan.ForSecrets(text)}
	var data any
	if err := json.Unmarshal([]byte(text), &data); err != nil {
		v.Verdict, v.Errors = "unreadable", []string{err.Error()}
		return v
	}
	v.Errors = format.ValidateRun(data)
	v.Verdict = "valid"
	if len(v.Errors) > 0 {
		v.Verdict = "invalid"
	}
	return v
}
