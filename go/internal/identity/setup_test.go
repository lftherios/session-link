package identity

import (
	"bytes"
	"context"
	"crypto/ecdh"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/json"
	"errors"
	"io"
	"net/http"

	"reflect"
	"strings"
	"testing"
)

// Forge a fully valid identity using only the public device information sent
// to the API. The server chooses a vault key and wraps it to the real device;
// accepting this record would disclose share keys on the next native backup.
func substitutedSetup(t *testing.T, submitted identityUpdate) *Record {
	t.Helper()
	var state State
	if err := submitted.Event.read(&state); err != nil {
		t.Fatal(err)
	}
	pub, root, err := ed25519.GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	recovery, err := ecdh.X25519().GenerateKey(rand.Reader)
	if err != nil {
		t.Fatal(err)
	}
	state.Root, state.Recovery = enc.EncodeToString(pub), enc.EncodeToString(recovery.PublicKey().Bytes())
	state.RecoveryBox, err = seal(random(32), append(root.Seed(), recovery.Bytes()...), "slink/recovery/v1/"+state.Account+"/"+state.Root)
	if err != nil {
		t.Fatal(err)
	}
	key := random(32)
	state.RecoveryWrap, err = wrap(state.Recovery, key, state.context())
	if err != nil {
		t.Fatal(err)
	}
	state.Devices[0].Wrap, err = wrap(state.Devices[0].Box, key, state.context())
	if err != nil {
		t.Fatal(err)
	}
	event, err := sign("event", state, root)
	if err != nil {
		t.Fatal(err)
	}
	data, err := seal(key, []byte(`{"version":1,"shares":[]}`), "slink/vault/v1/"+state.context())
	if err != nil {
		t.Fatal(err)
	}
	vault, err := sign("vault", Vault{Account: state.Account, Revision: 1, Head: event.hash(), Epoch: 1, Signer: "root", Data: data}, root)
	if err != nil {
		t.Fatal(err)
	}
	record := &Record{Events: []Signed{event}, Vault: vault}
	if _, _, err := record.verify(state.Account); err != nil {
		t.Fatal(err)
	}
	return record
}

// Calls remain synchronous, so these tests can inspect durable pins at the
// exact upload boundary without using real credentials or network services.
type setupTransport func(*http.Request) (*http.Response, error)

func (f setupTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }
func setupAPI(t *testing.T, api func(*identityUpdate) (remote, error)) Client {
	t.Helper()
	previous := http.DefaultTransport
	t.Cleanup(func() { http.DefaultTransport = previous })
	http.DefaultTransport = setupTransport(func(r *http.Request) (*http.Response, error) {
		if r.URL.String() != "https://setup.example.test/api/identity" {
			t.Fatalf("unexpected API URL: %s", r.URL)
		}
		var body *identityUpdate
		if r.Method == "POST" {
			body = &identityUpdate{}
			if err := json.NewDecoder(r.Body).Decode(body); err != nil {
				t.Fatal(err)
			}
		}
		response, err := api(body)
		if err != nil {
			return nil, err
		}
		raw, err := json.Marshal(response)
		if err != nil {
			t.Fatal(err)
		}
		return &http.Response{StatusCode: 200, Header: make(http.Header), Body: io.NopCloser(bytes.NewReader(raw))}, nil
	})
	return Client{Home: t.TempDir(), Server: "https://setup.example.test", APIKey: "fixture"}
}

func TestSetupRejectsServerIdentitySubstitution(t *testing.T) {
	for _, lostResponse := range []bool{false, true} {
		t.Run(map[bool]string{false: "acknowledgment", true: "after_restart"}[lostResponse], func(t *testing.T) {
			const account = "usr_setup"
			var stored *Record
			var original *localDevice
			var client Client
			posts := 0
			client = setupAPI(t, func(body *identityUpdate) (remote, error) {
				if body != nil {
					posts++
					var err error
					original, err = client.load(account)
					if err != nil {
						t.Fatal(err)
					}
					var state State
					if err := body.Event.read(&state); err != nil {
						t.Fatal(err)
					}
					if original == nil || original.Root != state.Root || original.Head != body.Event.hash() || !reflect.DeepEqual(original.PendingSetup, body) {
						t.Fatal("genesis was not durably saved before upload")
					}
					stored = substitutedSetup(t, *body)
					if lostResponse {
						return remote{}, errors.New("connection lost")
					}
				}
				return remote{Account: account, Record: stored}, nil
			})
			if _, err := client.Handle(context.Background(), "setup", Input{}); err == nil {
				t.Fatal("substitution accepted")
			}
			for _, action := range []string{"status", "setup", "confirm-recovery", "sync"} {
				if _, err := client.Handle(context.Background(), action, Input{}); err == nil || !strings.Contains(err.Error(), "recovery identity") {
					t.Fatalf("%s accepted substitution: %v", action, err)
				}
			}
			pinned, err := client.load(account)
			if err != nil {
				t.Fatal(err)
			}
			if !reflect.DeepEqual(original, pinned) {
				t.Fatal("untrusted response changed local identity")
			}
			if posts != 1 {
				t.Fatal("backup released keys after substitution")
			}
		})
	}
}

func TestSetupRetriesPreserveIdentity(t *testing.T) {
	for _, committed := range []bool{false, true} {
		t.Run(map[bool]string{false: "before_commit", true: "lost_acknowledgment"}[committed], func(t *testing.T) {
			const account = "usr_retry"
			var stored *Record
			var requests []identityUpdate
			client := setupAPI(t, func(body *identityUpdate) (remote, error) {
				if body != nil {
					requests = append(requests, *body)
					if committed || len(requests) > 1 {
						stored = &Record{Events: []Signed{*body.Event}, Vault: body.Vault}
					}
					if len(requests) == 1 {
						return remote{}, errors.New("connection lost")
					}
				}
				return remote{Account: account, Record: stored}, nil
			})
			if _, err := client.Handle(context.Background(), "setup", Input{}); err == nil {
				t.Fatal("expected interrupted setup")
			}
			pending, err := client.load(account)
			if err != nil {
				t.Fatal(err)
			}
			if pending.PendingSetup == nil || pending.Root == "" || pending.Head == "" || pending.Revision != 1 {
				t.Fatal("initial identity was not durably pinned")
			}
			// Handle reloads the durable record, as a fresh process would.
			status, err := client.Handle(context.Background(), "setup", Input{})
			if err != nil {
				t.Fatal(err)
			}
			if status.State != "ready" || status.RecoveryKey != pending.PendingRecovery {
				t.Fatal("retry changed the recovery key")
			}
			if !committed && (len(requests) != 2 || !reflect.DeepEqual(requests[0], requests[1])) {
				t.Fatal("retry changed the signed setup request")
			}
			if committed && len(requests) != 1 {
				t.Fatal("committed setup was uploaded again")
			}
			current, err := client.load(account)
			if err != nil {
				t.Fatal(err)
			}
			if current.PendingSetup != nil || current.Root != pending.Root || current.Head != pending.Head {
				t.Fatal("acknowledgment did not preserve the identity")
			}
			if _, err := client.Handle(context.Background(), "confirm-recovery", Input{}); err != nil {
				t.Fatal(err)
			}
			current, _ = client.load(account)
			if current.PendingRecovery != "" {
				t.Fatal("acknowledged recovery token was retained")
			}
			stored = nil
			if _, err := client.Handle(context.Background(), "setup", Input{}); err == nil {
				t.Fatal("confirmed identity was silently replaced")
			}
		})
	}
}

func TestSetupChecksLegacyPendingRecovery(t *testing.T) {
	const account = "usr_legacy"
	var stored *Record
	client := setupAPI(t, func(body *identityUpdate) (remote, error) {
		if body != nil {
			stored = substitutedSetup(t, *body)
			return remote{}, errors.New("connection lost")
		}
		return remote{Account: account, Record: stored}, nil
	})
	if _, err := client.Handle(context.Background(), "setup", Input{}); err == nil {
		t.Fatal("expected interrupted setup")
	}
	d, err := client.load(account)
	if err != nil {
		t.Fatal(err)
	}
	// Older clients saved only the recovery token before their first upload.
	d.Root, d.Head, d.Seq, d.Revision, d.PendingSetup = "", "", -1, 0, nil
	if err := client.save(d, account); err != nil {
		t.Fatal(err)
	}
	if _, err := client.Handle(context.Background(), "setup", Input{}); err == nil || !strings.Contains(err.Error(), "recovery identity") {
		t.Fatalf("legacy setup accepted substitution: %v", err)
	}
}
