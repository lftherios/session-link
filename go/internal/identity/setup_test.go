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
			for _, action := range []string{"status", "setup", "confirm-recovery", "sync", "discard-setup"} {
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
	for _, action := range []string{"setup", "discard-setup"} {
		if _, err := client.Handle(context.Background(), action, Input{}); err == nil || !strings.Contains(err.Error(), "recovery identity") {
			t.Fatalf("legacy %s accepted substitution: %v", action, err)
		}
	}
}

// An honest account shared by several devices: compare-and-swap writes and
// pending approval requests, as the server keeps them.
type sharedAccount struct {
	t        *testing.T
	record   *Record
	requests []Signed
	offline  bool
	posts    int
}

func (s *sharedAccount) client() Client {
	return Client{Home: s.t.TempDir(), Server: "https://setup.example.test", APIKey: "fixture"}
}
func newSharedAccount(t *testing.T, account string) *sharedAccount {
	t.Helper()
	s := &sharedAccount{t: t}
	previous := http.DefaultTransport
	t.Cleanup(func() { http.DefaultTransport = previous })
	reply := func(status int, value any) (*http.Response, error) {
		raw, err := json.Marshal(value)
		if err != nil {
			t.Fatal(err)
		}
		return &http.Response{StatusCode: status, Header: make(http.Header), Body: io.NopCloser(bytes.NewReader(raw))}, nil
	}
	http.DefaultTransport = setupTransport(func(r *http.Request) (*http.Response, error) {
		if r.Method == "POST" {
			if s.offline {
				return nil, errors.New("connection lost")
			}
			s.posts++
			var body struct {
				identityUpdate
				Request *Signed `json:"request"`
			}
			if err := json.NewDecoder(r.Body).Decode(&body); err != nil {
				t.Fatal(err)
			}
			if body.Request != nil {
				s.requests = append(s.requests, *body.Request)
			} else {
				next := Record{Vault: body.Vault}
				if s.record != nil {
					next.Events = append(next.Events, s.record.Events...)
				}
				if body.ExpectedHead != next.head() {
					return reply(409, map[string]any{"error": map[string]string{"message": "Vault changed; refresh and retry"}})
				}
				if body.Event != nil {
					next.Events = append(next.Events, *body.Event)
				}
				s.record = &next
			}
		}
		return reply(200, remote{Account: account, Record: s.record, Requests: s.requests})
	})
	return s
}

// A device whose own setup never reached the server must not stay locked out
// once another device has set the account up. It refuses that identity until
// the unfinished setup is discarded, then joins with fresh keys.
func TestSetupLostToAnotherDevice(t *testing.T) {
	for _, test := range []struct {
		name   string
		legacy bool
		join   string
	}{{"approval", false, "request"}, {"recovery_key", false, "recover"}, {"older_client", true, "recover"}} {
		t.Run(test.name, func(t *testing.T) {
			const account = "usr_race"
			ctx := context.Background()
			server := newSharedAccount(t, account)
			a, b := server.client(), server.client()
			server.offline = true
			if _, err := a.Handle(ctx, "setup", Input{Name: "A"}); err == nil {
				t.Fatal("expected interrupted setup")
			}
			server.offline = false
			unfinished, err := a.load(account)
			if err != nil || unfinished == nil {
				t.Fatal("interrupted setup was not saved", err)
			}
			if test.legacy {
				// Older clients saved only the recovery token before their first upload.
				unfinished.Root, unfinished.Head, unfinished.Seq, unfinished.Revision, unfinished.PendingSetup = "", "", -1, 0, nil
				if err := a.save(unfinished, account); err != nil {
					t.Fatal(err)
				}
			}
			first, err := b.Handle(ctx, "setup", Input{Name: "B"})
			if err != nil || first.State != "ready" || first.RecoveryKey == "" {
				t.Fatal("the other device could not set up", err)
			}
			posts := server.posts
			for _, action := range []string{"status", "setup", "request", "recover", "sync", "confirm-recovery"} {
				if _, err := a.Handle(ctx, action, Input{RecoveryKey: first.RecoveryKey}); !errors.Is(err, ErrSetupConflict) {
					t.Fatalf("%s did not report the unfinished setup: %v", action, err)
				}
			}
			if current, _ := a.load(account); !reflect.DeepEqual(unfinished, current) || server.posts != posts {
				t.Fatal("an undecided conflict changed local keys or uploaded")
			}
			status, err := a.Handle(ctx, "discard-setup", Input{})
			if err != nil || status.State != "locked" || status.DeviceID != "" || status.RecoveryPending {
				t.Fatalf("discard did not leave a new device: %+v %v", status, err)
			}
			if d, err := a.load(account); d != nil || err != nil {
				t.Fatal("discard kept the unfinished setup", err)
			}
			if test.join == "request" {
				pending, err := a.Handle(ctx, "request", Input{Name: "A"})
				if err != nil || pending.Confirmation == "" {
					t.Fatal("could not request approval", err)
				}
				if _, err := b.Handle(ctx, "approve", Input{ID: pending.DeviceID, Code: pending.Confirmation}); err != nil {
					t.Fatal(err)
				}
			} else if _, err := a.Handle(ctx, "recover", Input{Name: "A", RecoveryKey: first.RecoveryKey}); err != nil {
				t.Fatal(err)
			}
			status, err = a.Handle(ctx, "sync", Input{})
			if err != nil || status.State != "ready" || len(status.Devices) != 2 {
				t.Fatalf("device did not join the account: %+v %v", status, err)
			}
			joined, _ := a.load(account)
			if bytes.Equal(joined.Seed, unfinished.Seed) || bytes.Equal(joined.Box, unfinished.Box) || joined.PendingRecovery != "" {
				t.Fatal("discard kept keys the server had already seen")
			}
		})
	}
}

// Discarding is only for a setup that was never acknowledged. A device whose
// identity the server did acknowledge keeps refusing a replacement, and keeps
// the only copy of a recovery key it has not shown yet.
func TestDiscardSetupNeedsAnUnfinishedSetup(t *testing.T) {
	const account = "usr_keep"
	ctx := context.Background()
	server := newSharedAccount(t, account)
	a := server.client()
	if _, err := a.Handle(ctx, "setup", Input{}); err != nil {
		t.Fatal(err)
	}
	if _, err := a.Handle(ctx, "discard-setup", Input{}); err == nil || !strings.Contains(err.Error(), "no unfinished") {
		t.Fatalf("a finished setup was discarded: %v", err)
	}
	own, replaced := server.record, server.client()
	server.record = nil
	if _, err := replaced.Handle(ctx, "setup", Input{}); err != nil {
		t.Fatal(err)
	}
	for _, confirmed := range []bool{false, true} {
		saved, _ := a.load(account)
		if saved == nil || (saved.PendingRecovery == "") != confirmed {
			t.Fatal("unexpected recovery token state")
		}
		for _, action := range []string{"discard-setup", "status"} {
			if _, err := a.Handle(ctx, action, Input{}); err == nil || errors.Is(err, ErrSetupConflict) || !strings.Contains(err.Error(), "recovery identity") {
				t.Fatalf("%s accepted a replaced identity (confirmed=%v): %v", action, confirmed, err)
			}
		}
		if current, _ := a.load(account); !reflect.DeepEqual(saved, current) {
			t.Fatal("a replaced identity changed local keys")
		}
		if !confirmed {
			theirs := server.record
			server.record = own
			if _, err := a.Handle(ctx, "confirm-recovery", Input{}); err != nil {
				t.Fatal(err)
			}
			server.record = theirs
		}
	}
}
