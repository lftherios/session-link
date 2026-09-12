// Package identity keeps recovery secrets, device private keys, and vault
// plaintext in the native client. The service verifies signatures only.
package identity

import (
	"crypto/aes"
	"crypto/cipher"
	"crypto/ecdh"
	"crypto/ed25519"
	"crypto/hkdf"
	"crypto/rand"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
)

var enc = base64.RawURLEncoding
var ErrLocked = errors.New("this device needs approval or your recovery key")

func random(n int) []byte    { b := make([]byte, n); rand.Read(b); return b }
func digest(b []byte) string { h := sha256.Sum256(b); return enc.EncodeToString(h[:]) }
func decode(s string, n int) ([]byte, error) {
	b, err := enc.DecodeString(s)
	if err != nil || (n > 0 && len(b) != n) || enc.EncodeToString(b) != s {
		return nil, errors.New("invalid key encoding")
	}
	return b, nil
}
func seal(key, plain []byte, context string) (string, error) {
	block, err := aes.NewCipher(key)
	if err != nil {
		return "", err
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		return "", err
	}
	nonce := random(aead.NonceSize())
	return enc.EncodeToString(aead.Seal(nonce, nonce, plain, []byte(context))), nil
}
func unseal(key []byte, box, context string) ([]byte, error) {
	raw, err := decode(box, 0)
	if err != nil {
		return nil, err
	}
	block, err := aes.NewCipher(key)
	if err != nil {
		return nil, err
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		return nil, err
	}
	if len(raw) < aead.NonceSize()+aead.Overhead() {
		return nil, errors.New("truncated encrypted key")
	}
	return aead.Open(nil, raw[:aead.NonceSize()], raw[aead.NonceSize():], []byte(context))
}
func wrap(public string, key []byte, context string) (string, error) {
	raw, err := decode(public, 32)
	if err != nil {
		return "", err
	}
	pub, err := ecdh.X25519().NewPublicKey(raw)
	if err != nil {
		return "", err
	}
	ephemeral, err := ecdh.X25519().GenerateKey(rand.Reader)
	if err != nil {
		return "", err
	}
	secret, err := ephemeral.ECDH(pub)
	if err != nil {
		return "", err
	}
	info := "slink/wrap/v1/" + context + "/" + enc.EncodeToString(ephemeral.PublicKey().Bytes()) + "/" + public
	derived, err := hkdf.Key(sha256.New, secret, nil, info, 32)
	if err != nil {
		return "", err
	}
	box, err := seal(derived, key, info)
	if err != nil {
		return "", err
	}
	return enc.EncodeToString(ephemeral.PublicKey().Bytes()) + "." + box, nil
}
func unwrap(private []byte, box, context string) ([]byte, error) {
	if len(box) < 45 || box[43] != '.' {
		return nil, errors.New("invalid key wrapper")
	}
	raw, err := decode(box[:43], 32)
	if err != nil {
		return nil, err
	}
	pub, err := ecdh.X25519().NewPublicKey(raw)
	if err != nil {
		return nil, err
	}
	key, err := ecdh.X25519().NewPrivateKey(private)
	if err != nil {
		return nil, err
	}
	secret, err := key.ECDH(pub)
	if err != nil {
		return nil, err
	}
	info := "slink/wrap/v1/" + context + "/" + box[:43] + "/" + enc.EncodeToString(key.PublicKey().Bytes())
	derived, err := hkdf.Key(sha256.New, secret, nil, info, 32)
	if err != nil {
		return nil, err
	}
	out, err := unseal(derived, box[44:], info)
	if err != nil || len(out) != 32 {
		return nil, errors.New("cannot unlock vault key")
	}
	return out, nil
}

type Signed struct {
	Payload   string `json:"payload"`
	Signature string `json:"signature"`
}

func sign(kind string, value any, key ed25519.PrivateKey) (Signed, error) {
	b, err := json.Marshal(value)
	if err != nil {
		return Signed{}, err
	}
	return Signed{enc.EncodeToString(b), enc.EncodeToString(ed25519.Sign(key, append([]byte("slink/"+kind+"/v1\x00"), b...)))}, nil
}
func (s Signed) read(value any) error {
	raw, err := decode(s.Payload, 0)
	if err != nil {
		return err
	}
	return json.Unmarshal(raw, value)
}
func (s Signed) verify(kind, public string) error {
	raw, err := decode(s.Payload, 0)
	if err != nil {
		return err
	}
	pub, err := decode(public, 32)
	if err != nil {
		return err
	}
	sig, err := decode(s.Signature, 64)
	if err != nil {
		return err
	}
	if !ed25519.Verify(pub, append([]byte("slink/"+kind+"/v1\x00"), raw...), sig) {
		return fmt.Errorf("invalid %s signature", kind)
	}
	return nil
}
func (s Signed) hash() string { raw, _ := decode(s.Payload, 0); return digest(raw) }
