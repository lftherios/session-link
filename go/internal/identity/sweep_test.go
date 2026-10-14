package identity

import (
	"bufio"
	"encoding/base64"
	"encoding/json"
	"io"
	"os"
	"testing"

	"github.com/lftherios/session-link/internal/sealed"
)

// TestSweep is the Go side of scripts/protocol-diff.mjs. The script names a
// file of cases in SLINK_PROTOCOL_SWEEP, one JSON object on each line, and
// where the verdicts go in SLINK_PROTOCOL_VERDICTS. It is a test so that it
// reaches this package's verifier without the package exporting one, and it
// is skipped when the two variables are not set.
func TestSweep(t *testing.T) {
	in, out := os.Getenv("SLINK_PROTOCOL_SWEEP"), os.Getenv("SLINK_PROTOCOL_VERDICTS")
	if in == "" || out == "" {
		t.Skip("run by scripts/protocol-diff.mjs")
	}
	source, err := os.Open(in)
	if err != nil {
		t.Fatal(err)
	}
	defer source.Close()
	target, err := os.Create(out)
	if err != nil {
		t.Fatal(err)
	}
	defer target.Close()
	lines := bufio.NewReader(source)
	written := bufio.NewWriter(target)
	defer written.Flush()
	enc := json.NewEncoder(written)
	enc.SetEscapeHTML(false)
	for {
		line, err := lines.ReadBytes('\n')
		if len(line) > 1 {
			var c sweepCase
			if err := json.Unmarshal(line, &c); err != nil {
				t.Fatalf("a line is not a case: %v", err)
			}
			enc.Encode(c.judge())
		}
		if err == io.EOF {
			return
		}
		if err != nil {
			t.Fatal(err)
		}
	}
}

type sweepCase struct {
	T         string
	Envelope  string
	Key       string
	Kind      string
	Public    string
	Payload   string
	Signature string
	Account   string
	Events    []Signed
	Vault     *Signed
	Box       string
	Context   string
	Private   string
	Wrapper   string
}

type sweepVerdict struct {
	Verdict   string `json:"verdict"`
	Error     string `json:"error,omitempty"`
	Plaintext string `json:"plaintext,omitempty"`
	// A history's summary, or what a request says.
	State any `json:"state,omitempty"`
}

// sweepRequest is what a reader takes from a device request it verified.
type sweepRequest struct {
	ID      string `json:"id"`
	Account string `json:"account"`
	Root    string `json:"root"`
	Name    string `json:"name"`
	Created int64  `json:"created"`
	Code    string `json:"code"`
}

func (c sweepCase) judge() sweepVerdict {
	refused := func(verdict string, err error) sweepVerdict {
		return sweepVerdict{Verdict: verdict, Error: err.Error()}
	}
	switch c.T {
	case "envelope":
		data, _ := base64.StdEncoding.DecodeString(c.Envelope)
		plain, err := sealed.Decrypt(data, c.Key)
		if err != nil {
			return refused("refused", err)
		}
		return sweepVerdict{Verdict: "opens", Plaintext: base64.StdEncoding.EncodeToString(plain)}
	case "statement":
		if err := (Signed{Payload: c.Payload, Signature: c.Signature}).verify(c.Kind, c.Public); err != nil {
			return refused("refused", err)
		}
		return sweepVerdict{Verdict: "verified"}
	case "box":
		key, err := decode(c.Key, 32)
		if err != nil {
			return refused("refused", err)
		}
		plain, err := unseal(key, c.Box, c.Context)
		if err != nil {
			return refused("refused", err)
		}
		return sweepVerdict{Verdict: "opens", Plaintext: base64.StdEncoding.EncodeToString(plain)}
	case "wrapper":
		private, err := decode(c.Private, 32)
		if err != nil {
			return refused("refused", err)
		}
		key, err := unwrap(private, c.Wrapper, c.Context)
		if err != nil {
			return refused("refused", err)
		}
		return sweepVerdict{Verdict: "opens", Plaintext: base64.StdEncoding.EncodeToString(key)}
	case "request":
		// What handle does with a pending request before it looks at the
		// account, the root key and the time.
		signed := Signed{Payload: c.Payload, Signature: c.Signature}
		var request Request
		if err := signed.read(&request); err != nil {
			return refused("refused", err)
		}
		if err := signed.verify("request", request.Sign); err != nil {
			return refused("refused", err)
		}
		return sweepVerdict{Verdict: "verified", State: sweepRequest{request.id(), request.Account, request.Root, request.Name, request.Created, confirmation(signed)}}
	}
	var state State
	var vault Vault
	var err error
	if c.Vault != nil {
		state, vault, err = Record{Events: c.Events, Vault: *c.Vault}.verify(c.Account)
	} else {
		state, err = verifyHistory(c.Events, c.Account)
	}
	if err != nil {
		return refused("invalid", err)
	}
	return sweepVerdict{Verdict: "valid", State: summarize(c.Events, state, vault)}
}
