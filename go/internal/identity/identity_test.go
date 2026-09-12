package identity

import (
	"bufio"
	"context"
	"crypto/ecdh"
	"crypto/ed25519"
	"crypto/rand"
	"encoding/json"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestWrappingAndRecoveryIsolation(t *testing.T) {
	device, _ := ecdh.X25519().GenerateKey(rand.Reader)
	other, _ := ecdh.X25519().GenerateKey(rand.Reader)
	key := random(32)
	wrapped, err := wrap(enc.EncodeToString(device.PublicKey().Bytes()), key, "account/root/1")
	if err != nil {
		t.Fatal(err)
	}
	plain, err := unwrap(device.Bytes(), wrapped, "account/root/1")
	if err != nil || string(plain) != string(key) {
		t.Fatal("wrapper did not roundtrip", err)
	}
	for _, test := range []struct {
		private      []byte
		box, context string
	}{{other.Bytes(), wrapped, "account/root/1"}, {device.Bytes(), wrapped, "other/root/1"}, {device.Bytes(), wrapped, "account/root/2"}, {device.Bytes(), wrapped[:len(wrapped)-1], "account/root/1"}} {
		if _, err := unwrap(test.private, test.box, test.context); err == nil {
			t.Fatal("invalid wrapper accepted")
		}
	}
	secret := random(32)
	recovery, _ := seal(secret, random(64), "recovery/account/root")
	if _, err := unseal(random(32), recovery, "recovery/account/root"); err == nil {
		t.Fatal("wrong recovery key accepted")
	}
	if _, err := unseal(secret, recovery, "recovery/other/root"); err == nil {
		t.Fatal("recovery package crossed accounts")
	}
}
func TestSignatureDomainAndPayloadTampering(t *testing.T) {
	pub, key, _ := ed25519.GenerateKey(rand.Reader)
	signed, _ := sign("event", map[string]any{"account": "fixture"}, key)
	if err := signed.verify("event", enc.EncodeToString(pub)); err != nil {
		t.Fatal(err)
	}
	if err := signed.verify("vault", enc.EncodeToString(pub)); err == nil {
		t.Fatal("signature domain was not enforced")
	}
	signed.Payload = enc.EncodeToString([]byte(`{"account":"attacker"}`))
	if err := signed.verify("event", enc.EncodeToString(pub)); err == nil {
		t.Fatal("payload tampering accepted")
	}
}

func TestReceiptMigrationRequiresExactAccountOwnership(t *testing.T) {
	c := Client{Home: t.TempDir(), Server: "https://example.test"}
	key := enc.EncodeToString(random(32))
	legacy := Receipt{Server: c.Server, Key: key, SHA256: strings.Repeat("a", 64), URL: c.Server + "/s/23456789abcdef#key=" + key}
	if err := privateWrite(filepath.Join(c.Home, "shares", "legacy.json"), legacy); err != nil {
		t.Fatal(err)
	}
	other := legacy
	other.Account = "usr_other"
	other.URL = c.Server + "/s/23456789abcdeg#key=" + key
	if err := privateWrite(filepath.Join(c.Home, "shares", "other.json"), other); err != nil {
		t.Fatal(err)
	}
	data, err := c.merge(contents{}, "usr_me", nil)
	if err != nil || len(data.Shares) != 0 {
		t.Fatal("unscoped keys crossed into an unverified account", err)
	}
	data, err = c.merge(contents{}, "usr_me", []OwnedShare{{ID: "23456789abcdef", SHA256: strings.Repeat("b", 64)}})
	if err != nil || len(data.Shares) != 0 {
		t.Fatal("wrong ciphertext hash migrated", err)
	}
	data, err = c.merge(contents{}, "usr_me", []OwnedShare{{ID: "23456789abcdef", SHA256: legacy.SHA256}, {ID: "23456789abcdeg", SHA256: other.SHA256}})
	if err != nil || len(data.Shares) != 1 || data.Shares[0].Account != "usr_me" || data.Shares[0].URL != legacy.URL {
		t.Fatal("verified migration failed or included another account", err)
	}
}

// Run with SLINK_IDENTITY_BRIDGE pointing at the server's scripts/identity-bridge.mjs.
// This exercises the actual Node signature/CAS store with the native Go client.
func TestRecoveryDeviceIntegration(t *testing.T) {
	bridge := os.Getenv("SLINK_IDENTITY_BRIDGE")
	if bridge == "" {
		t.Skip("set SLINK_IDENTITY_BRIDGE for Go/Node protocol integration")
	}
	cmd := exec.Command("node", bridge)
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		t.Fatal(err)
	}
	cmd.Stderr = os.Stderr
	if err = cmd.Start(); err != nil {
		t.Fatal(err)
	}
	defer func() { cmd.Process.Kill(); cmd.Wait() }()
	scanner := bufio.NewScanner(stdout)
	if !scanner.Scan() {
		t.Fatal("bridge did not start")
	}
	var server struct{ URL, Dir string }
	if json.Unmarshal(scanner.Bytes(), &server) != nil {
		t.Fatal("invalid bridge output")
	}
	ctx := context.Background()
	a := Client{t.TempDir(), server.URL, "rk_fixture"}
	b := Client{t.TempDir(), server.URL, "rk_fixture"}
	c := Client{t.TempDir(), server.URL, "rk_fixture"}
	call := func(client Client, action string, in Input) Status {
		t.Helper()
		s, err := client.Handle(ctx, action, in)
		if err != nil {
			t.Fatalf("%s: %v", action, err)
		}
		return s
	}
	initial := call(a, "setup", Input{Name: "Laptop"})
	if initial.State != "ready" || initial.RecoveryKey == "" {
		t.Fatal("missing recovery key")
	}
	token := initial.RecoveryKey
	call(a, "confirm-recovery", Input{})
	d, _ := a.load(initial.Account)
	if d.PendingRecovery != "" {
		t.Fatal("recovery secret retained after acknowledgment")
	}
	addReceipt := func(client Client, id string) Receipt {
		t.Helper()
		key := enc.EncodeToString(random(32))
		r := Receipt{Server: server.URL, Account: initial.Account, Key: key, SHA256: strings.Repeat("a", 64), URL: server.URL + "/s/" + id + "#key=" + key}
		if err := privateWrite(filepath.Join(client.Home, "shares", id+".json"), r); err != nil {
			t.Fatal(err)
		}
		return r
	}
	first := addReceipt(a, "23456789abcdef")
	call(a, "sync", Input{})
	wrongAccount := Receipt{Server: server.URL, Account: "usr_other", Key: enc.EncodeToString(random(32)), SHA256: strings.Repeat("b", 64), URL: server.URL + "/s/23456789abcdeg#key=" + enc.EncodeToString(random(32))}
	privateWrite(filepath.Join(a.Home, "shares", "other.json"), wrongAccount)
	if s := call(b, "status", Input{}); s.State != "locked" || len(s.Shares) != 0 {
		t.Fatal("account login unlocked keys")
	}
	if _, err = b.Handle(ctx, "sync", Input{}); err != ErrLocked {
		t.Fatal("unapproved sync allowed", err)
	}
	request := call(b, "request", Input{Name: "Desktop"})
	if request.Confirmation == "" {
		t.Fatal("missing pairing code")
	}
	if _, err = a.Handle(ctx, "approve", Input{ID: request.DeviceID, Code: "WRONG"}); err == nil {
		t.Fatal("wrong code approved")
	}
	call(a, "approve", Input{ID: request.DeviceID, Code: request.Confirmation})
	unlocked := call(b, "status", Input{})
	if unlocked.State != "ready" || len(unlocked.Shares) != 1 || unlocked.Shares[0].URL != first.URL {
		t.Fatal("approved device failed to restore exact links")
	}
	before, _ := a.request(ctx, nil)
	oldState, oldVault, _ := before.Record.verify(initial.Account)
	oldB, _ := b.load(initial.Account)
	oldWrap, _ := oldState.device(request.DeviceID)
	oldKey, _ := unwrap(oldB.Box, oldWrap.Wrap, oldState.context())
	call(a, "revoke", Input{ID: request.DeviceID})
	after, _ := a.request(ctx, nil)
	newState, newVault, err := after.Record.verify(initial.Account)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := a.decrypt(oldKey, newState, newVault); err == nil {
		t.Fatal("revoked device unlocked rotated vault")
	}
	if s := call(b, "status", Input{}); s.State != "locked" || len(s.Shares) != 0 {
		t.Fatal("revoked device remained approved")
	}
	// A valid signature from the removed device cannot write a new snapshot.
	forged := newVault
	forged.Revision++
	forged.Signer = request.DeviceID
	signature, _ := sign("vault", forged, oldB.key())
	if _, err := a.request(ctx, map[string]any{"expected_head": after.Record.head(), "expected_revision": newVault.Revision, "vault": signature}); err == nil {
		t.Fatal("revoked writer accepted")
	}
	// Replaying a previously valid vault cannot overwrite the current version.
	if _, err := a.request(ctx, map[string]any{"expected_head": before.Record.head(), "expected_revision": oldVault.Revision, "vault": before.Record.Vault}); err == nil {
		t.Fatal("stale compare-and-swap accepted")
	}
	if _, err := c.Handle(ctx, "recover", Input{RecoveryKey: token[:len(token)-2] + "AA"}); err == nil {
		t.Fatal("wrong recovery secret accepted")
	}
	recovered := call(c, "recover", Input{Name: "Recovered laptop", RecoveryKey: token})
	if recovered.State != "ready" || len(recovered.Shares) != 1 || recovered.Shares[0].URL != first.URL {
		t.Fatal("recovery did not restore exact links")
	}
	second := addReceipt(c, "23456789abcdeh")
	call(c, "sync", Input{})
	if got := call(a, "status", Input{}); len(got.Shares) != 2 {
		t.Fatal("remaining approved device cannot read recovered-device additions")
	}
	if got := call(b, "status", Input{}); len(got.Shares) != 0 {
		t.Fatal("revoked device sees new keys")
	}
	// Local pins reject a legitimate but older history, and a substituted root.
	latest, _ := a.request(ctx, nil)
	latestState, latestVault, _ := latest.Record.verify(initial.Account)
	local, _ := a.load(initial.Account)
	if err := a.pin(local, *before.Record, oldState, oldVault); err == nil {
		t.Fatal("rollback accepted")
	}
	altered := latestState
	altered.Root = enc.EncodeToString(random(32))
	if err := a.pin(local, *latest.Record, altered, latestVault); err == nil {
		t.Fatal("root replacement accepted")
	}
	// Server storage contains wrappers and public metadata only.
	filepath.WalkDir(server.Dir, func(file string, entry os.DirEntry, err error) error {
		if err != nil {
			return err
		}
		if entry.IsDir() {
			return nil
		}
		raw, _ := os.ReadFile(file)
		for _, secret := range []string{first.Key, second.Key, token, enc.EncodeToString(d.Seed), enc.EncodeToString(oldKey)} {
			if strings.Contains(string(raw), secret) {
				t.Errorf("secret reached server storage: %s", entry.Name())
			}
		}
		return nil
	})
}
