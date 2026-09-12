package identity

import (
	"context"
	"crypto/ecdh"
	"crypto/ed25519"
	"crypto/hmac"
	"crypto/rand"
	"crypto/sha256"
	"encoding/json"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"
)

func TestNamedGrantRequiresInvitationProofAndApprovedHistory(t *testing.T) {
	_, root, _ := ed25519.GenerateKey(rand.Reader)
	device, _ := newDevice()
	inbox, _ := ecdh.X25519().GenerateKey(rand.Reader)
	recovery, _ := ecdh.X25519().GenerateKey(rand.Reader)
	state := State{Account: "usr_recipient", Kind: "init", Signer: "root", Root: enc.EncodeToString(root.Public().(ed25519.PublicKey)), Recovery: enc.EncodeToString(recovery.PublicKey().Bytes()), RecoveryBox: enc.EncodeToString(random(92)), Epoch: 1, Inbox: enc.EncodeToString(inbox.PublicKey().Bytes())}
	d := device.public("Recipient")
	d.Wrap, _ = wrap(d.Box, random(32), state.context())
	state.Devices = []Device{d}
	state.RecoveryWrap, _ = wrap(state.Recovery, random(32), state.context())
	event, _ := sign("event", state, root)
	owner, _ := newDevice()
	ownerRoot := enc.EncodeToString(random(32))
	invite := enc.EncodeToString(random(24))
	secret := random(32)
	saved := outbox{ID: "23456789abcdef", Account: "usr_owner", Root: ownerRoot, SHA256: strings.Repeat("a", 64), Key: enc.EncodeToString(random(32)), Invitations: []Invitation{{ID: invite, Email: "recipient@example.test", Commitment: digest(secret)}}, Secrets: map[string]string{invite: enc.EncodeToString(secret)}}
	binding := claim{Account: state.Account, Root: state.Root, Head: event.hash(), Signer: d.ID, ShareID: saved.ID, InviteID: invite, SHA256: saved.SHA256, Owner: saved.Account, OwnerRoot: saved.Root, Inbox: state.Inbox, Epoch: state.Epoch}
	signed, _ := sign("recipient-claim", binding, device.key())
	raw, _ := decode(signed.Payload, 0)
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte("slink/recipient-binding/v1\x00"))
	mac.Write(raw)
	valid := recipientClaim{Signed: signed, Proof: enc.EncodeToString(mac.Sum(nil))}
	submitted := valid
	var captured Signed
	posts := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.Method == "GET" {
			json.NewEncoder(w).Encode(map[string]any{"id": saved.ID, "owner": true, "sha256": saved.SHA256, "recipients": []any{map[string]any{"id": invite, "email": "recipient@example.test", "status": "accepted", "claim": submitted, "recipient_history": []Signed{event}}}})
			return
		}
		posts++
		var body struct {
			Grant Signed `json:"grant"`
		}
		json.NewDecoder(r.Body).Decode(&body)
		captured = body.Grant
		w.Write([]byte(`{"ok":true}`))
	}))
	defer server.Close()
	c := Client{Home: t.TempDir(), Server: server.URL, APIKey: "rk_fixture"}
	saved.Server = server.URL
	keys := nativeKeys{state: State{Account: saved.Account, Root: saved.Root}, device: owner, own: owner.public("Sender"), remote: remote{Record: &Record{Events: []Signed{event}}}}
	for _, attack := range []func(*recipientClaim){
		func(s *recipientClaim) { s.Proof = enc.EncodeToString(random(32)) },
		func(s *recipientClaim) { s.Signature = enc.EncodeToString(random(64)) },
		func(s *recipientClaim) { s.Payload = enc.EncodeToString([]byte(`{"account":"usr_attacker"}`)) },
	} {
		submitted = valid
		attack(&submitted)
		if _, err := c.finalizeNamed(context.Background(), saved, keys); err == nil {
			t.Fatal("unverified recipient received a grant")
		}
		if posts != 0 {
			t.Fatal("key was released before verifying invitation")
		}
	}
	submitted = valid
	if _, err := c.finalizeNamed(context.Background(), saved, keys); err != nil {
		t.Fatal(err)
	}
	if posts != 1 || captured.verify("named-grant", keys.own.Sign) != nil {
		t.Fatal("missing signed sender grant")
	}
	var grant namedGrant
	if captured.read(&grant) != nil || grant.ClaimHash != valid.hash() {
		t.Fatal("grant not bound to recipient claim")
	}
	opened, err := unwrap(inbox.Bytes(), grant.Wrap, namedContext(saved.SHA256, state.Account, state.Root, state.Epoch))
	if err != nil || enc.EncodeToString(opened) != saved.Key {
		t.Fatal("recipient cannot decrypt content key", err)
	}
}
func TestRevokeWithoutLocalOutboxIsNotSilentlyDropped(t *testing.T) {
	requests := 0
	server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		requests++
		w.Write([]byte(`{"account":"usr_owner"}`))
	}))
	defer server.Close()
	c := Client{Home: t.TempDir(), Server: server.URL, APIKey: "rk_fixture"}
	ctx := context.Background()
	if shares, err := c.NamedShares(ctx, "", "", true); err != nil || len(shares) != 0 || requests != 0 {
		t.Fatal("listing without a local outbox should stay offline", err)
	}
	if _, err := c.NamedShares(ctx, "not-a-share", "invite", true); err == nil || requests != 0 {
		t.Fatal("invalid share id accepted")
	}
	if _, err := c.NamedShares(ctx, "23456789abcdef", "", true); err == nil || requests != 0 {
		t.Fatal("revocation without an invitation accepted")
	}
	// Without a local outbox the old shortcut answered success before the
	// server ever heard about the revocation.
	if _, err := c.NamedShares(ctx, "23456789abcdef", "invite", true); err == nil || requests == 0 {
		t.Fatal("revocation reported success without reaching the server", err)
	}
}
func TestUnverifiableOutboxFilesNeverBlockVaultBackup(t *testing.T) {
	c := Client{Home: t.TempDir(), Server: "https://example.test"}
	state := State{Account: "usr_owner", Root: enc.EncodeToString(random(32))}
	invite, secret := enc.EncodeToString(random(24)), random(32)
	current := outbox{Server: c.Server, Account: state.Account, Root: state.Root, ID: "23456789abcdef", SHA256: strings.Repeat("a", 64), Key: enc.EncodeToString(random(32)), Invitations: []Invitation{{ID: invite, Email: "recipient@example.test", Commitment: digest(secret)}}, Secrets: map[string]string{invite: enc.EncodeToString(secret)}}
	// A share left behind by an identity this account has since replaced.
	previous := current
	previous.Root, previous.SHA256, previous.ID = enc.EncodeToString(random(32)), strings.Repeat("b", 64), "23456789abcdeg"
	for _, item := range []outbox{current, previous} {
		if err := privateWrite(c.outboxFile(item.SHA256), item); err != nil {
			t.Fatal(err)
		}
	}
	if err := privateWrite(c.outboxFile(strings.Repeat("c", 64)), []int{1}); err != nil {
		t.Fatal(err)
	}
	items, err := c.readOutbox(state.Account, state.Root)
	if err != nil || len(items) != 1 || items[0].SHA256 != current.SHA256 {
		t.Fatal("stale or corrupt files were not isolated from the verified outbox", err, len(items))
	}
	data, err := c.merge(contents{Version: 2}, state, nil)
	if err != nil || len(data.Named) != 1 {
		t.Fatal("stale local files changed the vault backup", err, len(data.Named))
	}
	// merge must never hand commit a backup it would reject.
	for _, item := range data.Named {
		if !validOutbox(item, c.Server, state.Account, state.Root) {
			t.Fatal("merged an entry that commit rejects")
		}
	}
	replaced := State{Account: state.Account, Root: enc.EncodeToString(random(32))}
	if data, err := c.merge(contents{}, replaced, nil); err != nil || len(data.Named) != 0 || data.Version != 1 {
		t.Fatal("previous-identity shares leaked into a fresh vault", err, len(data.Named))
	}
}
