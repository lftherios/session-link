package identity

import (
	"encoding/json"
	"fmt"
	"os"
	"reflect"
	"testing"
)

// The cases every implementation of the protocol shares
// (testdata/protocol/README.md). The viewer reads the same files in
// test/protocol-cases.test.mjs and Rust in crates/slink-protocol/tests.

// expectation is one verdict for every implementation, or one for each.
type expectation struct{ JS, Go, Rust string }

func (e *expectation) UnmarshalJSON(raw []byte) error {
	var all string
	if json.Unmarshal(raw, &all) == nil {
		*e = expectation{all, all, all}
		return nil
	}
	type each expectation
	return json.Unmarshal(raw, (*each)(e))
}

type summaryDevice struct {
	ID   string `json:"id"`
	Name string `json:"name"`
}

// summary is what a verifier must report of a history it accepts.
type summary struct {
	Head     string          `json:"head"`
	Seq      int             `json:"seq"`
	Kind     string          `json:"kind"`
	Signer   string          `json:"signer"`
	Epoch    int             `json:"epoch"`
	Inbox    string          `json:"inbox"`
	Devices  []summaryDevice `json:"devices"`
	Revision int             `json:"revision,omitempty"`
}

type historyCase struct {
	Name    string
	Account string
	Events  []string
	Vault   string
	Expect  expectation
	State   json.RawMessage
}

// judge is what this package makes of a case: the state it reads, or why not.
func judge(signed map[string]Signed, c historyCase) (*summary, error) {
	events := make([]Signed, 0, len(c.Events))
	for _, id := range c.Events {
		event, ok := signed[id]
		if !ok {
			return nil, fmt.Errorf("the case names a statement the file does not hold: %s", id)
		}
		events = append(events, event)
	}
	var state State
	var vault Vault
	var err error
	if c.Vault != "" {
		state, vault, err = Record{Events: events, Vault: signed[c.Vault]}.verify(c.Account)
	} else {
		state, err = verifyHistory(events, c.Account)
	}
	if err != nil {
		return nil, err
	}
	return summarize(events, state, vault), nil
}

func summarize(events []Signed, state State, vault Vault) *summary {
	out := &summary{Head: events[len(events)-1].hash(), Seq: state.Seq, Kind: state.Kind, Signer: state.Signer, Epoch: state.Epoch, Inbox: state.Inbox, Revision: vault.Revision}
	for _, d := range state.Devices {
		out.Devices = append(out.Devices, summaryDevice{d.ID, d.Name})
	}
	return out
}

func TestHistoryCases(t *testing.T) {
	raw, err := os.ReadFile("../../../testdata/protocol/histories.json")
	if err != nil {
		t.Fatal(err)
	}
	var file struct {
		Signed map[string]Signed
		Cases  []historyCase
	}
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	valid, invalid := 0, 0
	for _, c := range file.Cases {
		got, err := judge(file.Signed, c)
		switch c.Expect.Go {
		case "invalid":
			invalid++
			if err == nil {
				t.Errorf("%s: accepted", c.Name)
			}
		case "valid":
			valid++
			if err != nil {
				t.Errorf("%s: %v", c.Name, err)
				continue
			}
			// One state for every implementation that accepts, or one for each.
			var each struct{ Go *summary }
			var want summary
			if json.Unmarshal(c.State, &each); each.Go != nil {
				want = *each.Go
			} else if err := json.Unmarshal(c.State, &want); err != nil {
				t.Errorf("%s: the case has no state: %v", c.Name, err)
				continue
			}
			if !reflect.DeepEqual(*got, want) {
				t.Errorf("%s: read %+v, want %+v", c.Name, *got, want)
			}
		default:
			t.Errorf("%s: unknown expectation %q", c.Name, c.Expect.Go)
		}
	}
	if valid < 20 || invalid < 100 {
		t.Fatalf("%d valid and %d invalid cases; the file has lost most of them", valid, invalid)
	}
}

func TestStatementCases(t *testing.T) {
	raw, err := os.ReadFile("../../../testdata/protocol/statements.json")
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		Name, Kind, Public, Payload, Signature string
		Expect                                 expectation
	}
	if err := json.Unmarshal(raw, &cases); err != nil {
		t.Fatal(err)
	}
	verified := 0
	for _, c := range cases {
		err := Signed{Payload: c.Payload, Signature: c.Signature}.verify(c.Kind, c.Public)
		switch c.Expect.Go {
		case "verified":
			verified++
			if err != nil {
				t.Errorf("%s: %v", c.Name, err)
			}
		case "refused":
			if err == nil {
				t.Errorf("%s: verified", c.Name)
			}
		default:
			t.Errorf("%s: unknown expectation %q", c.Name, c.Expect.Go)
		}
	}
	if verified == 0 || verified == len(cases) {
		t.Fatalf("%d of %d cases verify; both kinds are needed", verified, len(cases))
	}
}
