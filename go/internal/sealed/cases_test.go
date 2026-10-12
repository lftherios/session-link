package sealed

import (
	"encoding/base64"
	"encoding/json"
	"os"
	"testing"
)

// TestEnvelopeCases holds Decrypt to the envelope cases every implementation
// shares (testdata/protocol/README.md).
func TestEnvelopeCases(t *testing.T) {
	raw, err := os.ReadFile("../../../testdata/protocol/envelopes.json")
	if err != nil {
		t.Fatal(err)
	}
	var cases []struct {
		Name, Envelope, Key, Expect, Plaintext string
	}
	if err := json.Unmarshal(raw, &cases); err != nil {
		t.Fatal(err)
	}
	opened := 0
	for _, c := range cases {
		envelope, err := base64.StdEncoding.DecodeString(c.Envelope)
		if err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		plain, err := Decrypt(envelope, c.Key)
		switch c.Expect {
		case "opens":
			opened++
			if err != nil || base64.StdEncoding.EncodeToString(plain) != c.Plaintext {
				t.Errorf("%s: did not open to its document: %v", c.Name, err)
			}
		case "refused":
			if err == nil {
				t.Errorf("%s: opened", c.Name)
			}
		default:
			t.Errorf("%s: unknown expectation %q", c.Name, c.Expect)
		}
	}
	if opened == 0 || opened == len(cases) {
		t.Fatalf("%d of %d cases open; both kinds are needed", opened, len(cases))
	}
}
