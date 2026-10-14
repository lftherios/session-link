package identity

import (
	"bytes"
	"crypto/ecdh"
	"crypto/ed25519"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"os"
	"strings"
	"testing"
)

// The boxes, wrappers, signatures, device requests, accounts opened by a
// device and by a recovery key, and shares for named people, that every
// implementation shares (testdata/protocol/README.md). The viewer reads the same file in
// test/protocol-cases.test.mjs and Rust in crates/slink-protocol/tests/keys.rs.
//
// This package makes its nonces and one-time keys itself, so it cannot make a
// case again as Rust does. It opens each one, and where a statement is what a
// client writes it signs the same bytes.

type keyRecord struct {
	Account string
	Events  []Signed
	Vault   Signed
}

type keyFile struct {
	Boxes []struct {
		Name, Key, Context, Box, Plaintext string
		Expect                             expectation
	}
	Wrappers []struct {
		Name, Private, Public, Context, Wrapper, Key string
		Expect                                       expectation
	}
	Recipients []struct {
		Name, Public string
		Expect       expectation
	}
	Signatures []struct {
		Name, Kind, Seed, Public, Payload, Signature string
	}
	Requests []struct {
		Name, Payload, Signature, ID, Code, Seed string
		Expect                                   expectation
	}
	Devices []struct {
		keyRecord
		Name, Device, Private, Contents string
		VaultKey                        string `json:"vault_key"`
		Expect                          expectation
	}
	Recoveries []struct {
		keyRecord
		Name, Token, Root, Agreement, Contents string
		VaultKey                               string `json:"vault_key"`
		Expect                                 expectation
	}
	Shares []shareCase
}

// shareCase is what a sender holds of a share for named people when a
// recipient's claim arrives, and the grant it must then make.
type shareCase struct {
	Name  string
	Share struct {
		ID     string
		SHA256 string
	}
	Key        string
	Invitation struct {
		Invitation
		Secret string
	}
	Owner struct {
		Account, Root, Head, Signer, Seed string
		History                           []Signed
		Invited                           int
		Inboxes                           map[string]string
	}
	Invite    Signed
	Claim     recipientClaim
	Recipient struct {
		Account string
		History []Signed
		Inboxes map[string]string
	}
	Expect expectation
	Grant  Signed
}

func readKeyCases(t *testing.T) keyFile {
	t.Helper()
	raw, err := os.ReadFile("../../../testdata/protocol/keys.json")
	if err != nil {
		t.Fatal(err)
	}
	var file keyFile
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	return file
}

func mustKey(t *testing.T, name, text string) []byte {
	t.Helper()
	raw, err := decode(text, 32)
	if err != nil {
		t.Fatalf("%s: the case's key is not one: %v", name, err)
	}
	return raw
}

// settle compares what this package made of a case with what the case says,
// and counts the ones that open.
func settle(t *testing.T, name, opens string, expect expectation, err error, opened *int) bool {
	t.Helper()
	switch expect.Go {
	case opens:
		*opened++
		if err != nil {
			t.Errorf("%s: %v", name, err)
			return false
		}
		return true
	case "refused":
		if err == nil {
			t.Errorf("%s: want refused, got %s", name, opens)
		}
	default:
		t.Errorf("%s: unknown expectation %q", name, expect.Go)
	}
	return false
}

func bothKinds(t *testing.T, what string, opened, all int) {
	t.Helper()
	if opened == 0 || opened == all {
		t.Fatalf("%d of %d %s are accepted; both kinds are needed", opened, all, what)
	}
}

func TestBoxCases(t *testing.T) {
	file := readKeyCases(t)
	opened := 0
	for _, c := range file.Boxes {
		plain, err := unseal(mustKey(t, c.Name, c.Key), c.Box, c.Context)
		if settle(t, c.Name, "opens", c.Expect, err, &opened) {
			if base64.StdEncoding.EncodeToString(plain) != c.Plaintext {
				t.Errorf("%s: opened to %x", c.Name, plain)
			}
		}
	}
	bothKinds(t, "boxes", opened, len(file.Boxes))
}

func TestWrapperCases(t *testing.T) {
	file := readKeyCases(t)
	opened := 0
	for _, c := range file.Wrappers {
		private := mustKey(t, c.Name, c.Private)
		key, err := unwrap(private, c.Wrapper, c.Context)
		if settle(t, c.Name, "opens", c.Expect, err, &opened) {
			if enc.EncodeToString(key) != c.Key {
				t.Errorf("%s: unwrapped %x", c.Name, key)
			}
		}
		own, _ := ecdh.X25519().NewPrivateKey(private)
		if public := enc.EncodeToString(own.PublicKey().Bytes()); public != c.Public {
			t.Errorf("%s: the private key's public key is %s", c.Name, public)
		}
	}
	bothKinds(t, "wrappers", opened, len(file.Wrappers))

	wrapped := 0
	for _, c := range file.Recipients {
		made, err := wrap(c.Public, make([]byte, 32), "context")
		if settle(t, c.Name, "wraps", c.Expect, err, &wrapped) && len(made) != 124 {
			t.Errorf("%s: a wrapper of %d characters", c.Name, len(made))
		}
	}
	bothKinds(t, "keys to wrap to", wrapped, len(file.Recipients))
}

func TestSignatureCases(t *testing.T) {
	file := readKeyCases(t)
	kinds := map[string]bool{}
	for _, c := range file.Signatures {
		key := ed25519.NewKeyFromSeed(mustKey(t, c.Name, c.Seed))
		if public := enc.EncodeToString(key.Public().(ed25519.PublicKey)); public != c.Public {
			t.Errorf("%s: the seed's public key is %s", c.Name, public)
		}
		raw, err := decode(c.Payload, 0)
		if err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		// The payload is written without white space or anything Go would
		// escape, so marshalling it again leaves its bytes as they are.
		made, err := sign(c.Kind, json.RawMessage(raw), key)
		if err != nil || made != (Signed{c.Payload, c.Signature}) {
			t.Errorf("%s: signed %+v, %v", c.Name, made, err)
		}
		if err := made.verify(c.Kind, c.Public); err != nil {
			t.Errorf("%s: %v", c.Name, err)
		}
		kinds[c.Kind] = true
	}
	if len(kinds) != 7 {
		t.Fatalf("the cases sign as %d kinds of statement, not seven", len(kinds))
	}
}

func TestRequestCases(t *testing.T) {
	file := readKeyCases(t)
	verified, made := 0, 0
	for _, c := range file.Requests {
		signed := Signed{c.Payload, c.Signature}
		// What handle does with a pending request before it looks at the
		// account, the root key and the time.
		var request Request
		err := signed.read(&request)
		if err == nil {
			err = signed.verify("request", request.Sign)
		}
		if !settle(t, c.Name, "verified", c.Expect, err, &verified) {
			continue
		}
		// A case only some implementations verify says nothing more of it.
		if c.ID == "" {
			continue
		}
		if request.id() != c.ID {
			t.Errorf("%s: the device would be %s", c.Name, request.id())
		}
		code := confirmation(signed)
		if code != c.Code || normalizeCode(strings.ToLower(strings.ReplaceAll(code, "-", " "))) != normalizeCode(c.Code) {
			t.Errorf("%s: the code is %s", c.Name, code)
		}
		if c.Seed == "" {
			continue
		}
		made++
		again, err := sign("request", request, ed25519.NewKeyFromSeed(mustKey(t, c.Name, c.Seed)))
		if err != nil || again != signed {
			t.Errorf("%s: signed again as %+v, %v", c.Name, again, err)
		}
	}
	bothKinds(t, "requests", verified, len(file.Requests))
	if made < 5 {
		t.Fatalf("%d requests were signed again", made)
	}
}

// openVault is what a client does once it holds the vault key.
func openVault(key []byte, state State, vault Vault) ([]byte, error) {
	return unseal(key, vault.Data, "slink/vault/v1/"+state.context())
}

// checkVault compares an opened vault with what its case says, and holds the
// vaults a client would write to the client's own reading of them.
func checkVault(t *testing.T, name string, record keyRecord, state State, vault Vault, key, contents []byte, wantKey, wantContents string) {
	t.Helper()
	if enc.EncodeToString(key) != wantKey {
		t.Errorf("%s: the vault key is %x", name, key)
	}
	if base64.StdEncoding.EncodeToString(contents) != wantContents {
		t.Errorf("%s: the vault holds %q", name, contents)
	}
	if written, _ := json.Marshal(vault); enc.EncodeToString(written) != record.Vault.Payload {
		t.Errorf("%s: the vault statement would be written as %s", name, written)
	}
	if !json.Valid(contents) {
		return
	}
	read, err := Client{Server: "https://session.example"}.decrypt(key, state, vault)
	if err != nil || len(read.Shares) != 0 {
		t.Errorf("%s: the client reads the vault as %+v, %v", name, read, err)
	}
}

func TestDeviceCases(t *testing.T) {
	file := readKeyCases(t)
	opened := 0
	for _, c := range file.Devices {
		state, vault, err := Record{Events: c.Events, Vault: c.Vault}.verify(c.Account)
		if err != nil {
			t.Fatalf("%s: the record does not verify: %v", c.Name, err)
		}
		var key, contents []byte
		device, ok := state.device(c.Device)
		if !ok {
			err = ErrLocked
		} else if key, err = unwrap(mustKey(t, c.Name, c.Private), device.Wrap, state.context()); err == nil {
			contents, err = openVault(key, state, vault)
		}
		if settle(t, c.Name, "opens", c.Expect, err, &opened) {
			checkVault(t, c.Name, c.keyRecord, state, vault, key, contents, c.VaultKey, c.Contents)
		}
	}
	bothKinds(t, "devices", opened, len(file.Devices))
}

// recoverWith is the recovery branch of handle, from the key a person typed
// to the vault key. The rule lives inside the code that talks to the service,
// so it is repeated here with this package's own primitives.
func recoverWith(token string, state State) (private, key []byte, err error) {
	parts := strings.Split(strings.TrimSpace(token), ".")
	if len(parts) != 3 || parts[0] != "slr1" || parts[1] != digest([]byte(state.Account+"\x00"+state.Root)) {
		return nil, nil, errors.New("recovery key does not match this account's recovery identity")
	}
	secret, err := decode(parts[2], 32)
	if err != nil {
		return nil, nil, err
	}
	private, err = unseal(secret, state.RecoveryBox, "slink/recovery/v1/"+state.Account+"/"+state.Root)
	if err != nil || len(private) != 64 {
		return nil, nil, errors.New("incorrect recovery key")
	}
	root := ed25519.NewKeyFromSeed(private[:32])
	recovery, err := ecdh.X25519().NewPrivateKey(private[32:])
	if err != nil || enc.EncodeToString(root.Public().(ed25519.PublicKey)) != state.Root || enc.EncodeToString(recovery.PublicKey().Bytes()) != state.Recovery {
		return nil, nil, errors.New("recovery identity verification failed")
	}
	key, err = unwrap(private[32:], state.RecoveryWrap, state.context())
	return private, key, err
}

func TestRecoveryCases(t *testing.T) {
	file := readKeyCases(t)
	opened := 0
	for _, c := range file.Recoveries {
		state, vault, err := Record{Events: c.Events, Vault: c.Vault}.verify(c.Account)
		if err != nil {
			t.Fatalf("%s: the record does not verify: %v", c.Name, err)
		}
		var contents []byte
		private, key, err := recoverWith(c.Token, state)
		if err == nil {
			contents, err = openVault(key, state, vault)
		}
		if !settle(t, c.Name, "opens", c.Expect, err, &opened) {
			continue
		}
		if !bytes.Equal(private[:32], mustKey(t, c.Name, c.Root)) || !bytes.Equal(private[32:], mustKey(t, c.Name, c.Agreement)) {
			t.Errorf("%s: the package holds other keys", c.Name)
		}
		checkVault(t, c.Name, c.keyRecord, state, vault, key, contents, c.VaultKey, c.Contents)
		// The key a pending setup keeps is compared as it was stored.
		pending := localDevice{PendingRecovery: strings.TrimSpace(c.Token)}
		if !pending.recoveryMatches(state) {
			t.Errorf("%s: the stored recovery key does not match its own identity", c.Name)
		}
	}
	bothKinds(t, "recovery keys", opened, len(file.Recoveries))
}

// A history spelt as a client spells it is written back byte for byte: this
// package and the Rust crate write a state, and a vault, the same way.
func TestClientSpelling(t *testing.T) {
	raw, err := os.ReadFile("../../../testdata/protocol/histories.json")
	if err != nil {
		t.Fatal(err)
	}
	var file struct {
		Signed map[string]Signed
		Cases  []struct {
			Name   string
			Events []string
			Vault  string
			Client bool
		}
	}
	if err := json.Unmarshal(raw, &file); err != nil {
		t.Fatal(err)
	}
	states, vaults := 0, 0
	same := func(name string, signed Signed, value any) {
		if err := signed.read(value); err != nil {
			t.Fatalf("%s: %v", name, err)
		}
		if written, _ := json.Marshal(value); enc.EncodeToString(written) != signed.Payload {
			t.Errorf("%s: written back as %s", name, written)
		}
	}
	for _, c := range file.Cases {
		if !c.Client {
			continue
		}
		for _, id := range c.Events {
			same(c.Name, file.Signed[id], &State{})
			states++
		}
		if c.Vault != "" {
			same(c.Name, file.Signed[c.Vault], &Vault{})
			vaults++
		}
	}
	if states < 20 || vaults < 5 {
		t.Fatalf("%d states and %d vaults were written back", states, vaults)
	}
}

// judgeClaim is what finalizeNamed checks of an accepted invitation before it
// wraps the share's key: the recipient's state to wrap to, or why not. The
// rule lives inside the code that talks to the service, so it is repeated
// here with this package's own primitives.
func judgeClaim(c shareCase) (State, error) {
	signed := c.Claim
	var binding claim
	if signed.read(&binding) != nil {
		return State{}, errors.New("invalid recipient identity binding")
	}
	raw, _ := decode(signed.Payload, 0)
	secret, err := decode(c.Invitation.Secret, 32)
	if err != nil {
		return State{}, err
	}
	mac := hmac.New(sha256.New, secret)
	mac.Write([]byte("slink/recipient-binding/v1\x00"))
	mac.Write(raw)
	submitted, err := decode(signed.Proof, 32)
	if err != nil || !hmac.Equal(mac.Sum(nil), submitted) || binding.InviteID != c.Invitation.ID || binding.ShareID != c.Share.ID || binding.SHA256 != c.Share.SHA256 || binding.Owner != c.Owner.Account || binding.OwnerRoot != c.Owner.Root {
		return State{}, errors.New("recipient verification failed; no decryption key was released")
	}
	state, err := verifyHistory(c.Recipient.History, binding.Account)
	if err != nil {
		return State{}, err
	}
	if state.Root != binding.Root || state.Inbox == "" {
		return State{}, errors.New("recipient's encryption identity changed")
	}
	index := -1
	for i, event := range c.Recipient.History {
		if event.hash() == binding.Head {
			index = i
			break
		}
	}
	if index < 0 {
		return State{}, errors.New("recipient history conflicts with its invitation")
	}
	then, err := verifyHistory(c.Recipient.History[:index+1], binding.Account)
	if err != nil {
		return State{}, err
	}
	device, ok := then.device(binding.Signer)
	if !ok || then.Inbox != binding.Inbox || then.Epoch != binding.Epoch || signed.verify("recipient-claim", device.Sign) != nil {
		return State{}, errors.New("recipient request is not from an approved device")
	}
	return state, nil
}

// opensFor checks that a share's wrapper opens to its key for the account it
// was made for, with the private key the case gives for that account.
func opensFor(t *testing.T, name string, c shareCase, inboxes map[string]string, state State, wrapper string) {
	t.Helper()
	private := mustKey(t, name, inboxes[state.Inbox])
	key, err := unwrap(private, wrapper, namedContext(c.Share.SHA256, state.Account, state.Root, state.Epoch))
	if err != nil || enc.EncodeToString(key) != c.Key {
		t.Errorf("%s: the wrapper for %s opens to %x, %v", name, state.Account, key, err)
	}
}

func TestShareCases(t *testing.T) {
	file := readKeyCases(t)
	granted := 0
	for _, c := range file.Shares {
		signer := ed25519.NewKeyFromSeed(mustKey(t, c.Name, c.Owner.Seed))

		// The invitation: a statement this package writes the same way, a
		// wrapper its sender can open, and an outbox entry it would keep.
		var invited invite
		if err := c.Invite.read(&invited); err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		if again, err := sign("named-invite", invited, signer); err != nil || again != c.Invite {
			t.Errorf("%s: the invitation signed again is %+v, %v", c.Name, again, err)
		}
		owner, err := verifyHistory(c.Owner.History[:c.Owner.Invited], c.Owner.Account)
		if err != nil {
			t.Fatalf("%s: the sender's history: %v", c.Name, err)
		}
		opensFor(t, c.Name, c, c.Owner.Inboxes, owner, invited.Wrap)
		saved := outbox{Server: "https://session.example", Account: c.Owner.Account, Root: c.Owner.Root, ID: c.Share.ID, SHA256: c.Share.SHA256, Key: c.Key, Invitations: []Invitation{c.Invitation.Invitation}, Secrets: map[string]string{c.Invitation.ID: c.Invitation.Secret}}
		if !validOutbox(saved, saved.Server, saved.Account, saved.Root) {
			t.Errorf("%s: the invitation is not one this package would keep", c.Name)
		}

		state, err := judgeClaim(c)
		if !settle(t, c.Name, "granted", c.Expect, err, &granted) {
			continue
		}
		// A case only some implementations grant says nothing more of it.
		if c.Grant.Payload == "" {
			continue
		}
		var want namedGrant
		if err := c.Grant.read(&want); err != nil {
			t.Fatalf("%s: %v", c.Name, err)
		}
		// The grant as finalizeNamed fills it in, around the wrapper the
		// case made: this package would choose a one-time key of its own.
		grant := namedGrant{Account: c.Owner.Account, Root: c.Owner.Root, Head: c.Owner.Head, Signer: c.Owner.Signer, ShareID: c.Share.ID, InviteID: c.Invitation.ID, SHA256: c.Share.SHA256, Recipient: state.Account, RecipientRoot: state.Root, Inbox: state.Inbox, Epoch: state.Epoch, Wrap: want.Wrap, ClaimHash: c.Claim.hash()}
		if again, err := sign("named-grant", grant, signer); err != nil || again != c.Grant {
			t.Errorf("%s: the grant signed again is %+v, %v", c.Name, again, err)
		}
		opensFor(t, c.Name, c, c.Recipient.Inboxes, state, want.Wrap)
	}
	bothKinds(t, "claims", granted, len(file.Shares))
}
