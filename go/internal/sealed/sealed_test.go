package sealed

import (
	"bytes"
	"encoding/base64"
	"encoding/json"
	"os"
	"testing"
)

func TestEnvelopeAuthentication(t *testing.T) {
	plain := []byte("private title and session contents\x00\xff")
	data, key, err := Encrypt(plain)
	if err != nil {
		t.Fatal(err)
	}
	other, otherKey, _ := Encrypt(plain)
	if key == otherKey || bytes.Equal(data, other) {
		t.Fatal("encryption is not randomized")
	}
	got, err := Decrypt(data, key)
	if err != nil || !bytes.Equal(got, plain) {
		t.Fatal("roundtrip", err)
	}
	if _, err = Decrypt(data, otherKey); err == nil {
		t.Fatal("wrong key accepted")
	}
	for i := range data {
		changed := bytes.Clone(data)
		changed[i] ^= 1
		if _, err := Decrypt(changed, key); err == nil {
			t.Fatalf("tampered byte %d accepted", i)
		}
	}
	for n := 0; n < len(data); n++ {
		if _, err := Decrypt(data[:n], key); err == nil {
			t.Fatalf("truncation %d accepted", n)
		}
	}
	if _, err := Decrypt(append(bytes.Clone(data), 0), key); err == nil {
		t.Fatal("trailing bytes accepted")
	}
	if _, _, err := Encrypt(make([]byte, MaxPlaintext+1)); err == nil {
		t.Fatal("oversized document accepted")
	}
}

func TestWebCryptoFixture(t *testing.T) {
	raw, err := os.ReadFile("../../../testdata/encryption/webcrypto.json")
	if err != nil {
		t.Fatal(err)
	}
	var fixture struct{ Key, Envelope, Plaintext string }
	if err := json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	envelope, _ := base64.StdEncoding.DecodeString(fixture.Envelope)
	plain, err := Decrypt(envelope, fixture.Key)
	if err != nil || string(plain) != fixture.Plaintext {
		t.Fatal("Web Crypto interoperability failed", err)
	}
}
