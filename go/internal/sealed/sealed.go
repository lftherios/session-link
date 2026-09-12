// Package sealed defines the session.link encrypted document envelope.
// Keys and plaintext never belong in a hosted API request.
package sealed

import (
	"bytes"
	"crypto/aes"
	"crypto/cipher"
	"crypto/rand"
	"encoding/base64"
	"errors"
)

const MaxPlaintext = 25 * 1024 * 1024
const HeaderSize = 24
const MaxEnvelope = HeaderSize + MaxPlaintext + 16

// Eight magic/version bytes, four reserved zero bytes, then a 12-byte nonce.
// The complete header is authenticated as additional data.
var magic = []byte{'S', 'L', 'I', 'N', 'K', 'E', 0, 1}

func Validate(data []byte) error {
	if len(data) < HeaderSize+16 || len(data) > MaxEnvelope || !bytes.Equal(data[:8], magic) || !bytes.Equal(data[8:12], make([]byte, 4)) {
		return errors.New("invalid or unsupported encrypted session envelope")
	}
	return nil
}

func Encrypt(plaintext []byte) (data []byte, key string, err error) {
	if len(plaintext) > MaxPlaintext {
		return nil, "", errors.New("session exceeds the 25 MiB limit")
	}
	k := make([]byte, 32)
	if _, err = rand.Read(k); err != nil {
		return
	}
	header := make([]byte, HeaderSize)
	copy(header, magic)
	if _, err = rand.Read(header[12:]); err != nil {
		return
	}
	block, err := aes.NewCipher(k)
	if err != nil {
		return
	}
	aead, err := cipher.NewGCM(block)
	if err != nil {
		return
	}
	data = aead.Seal(header, header[12:], plaintext, header)
	key = base64.RawURLEncoding.EncodeToString(k)
	return
}

func Decrypt(data []byte, key string) ([]byte, error) {
	if err := Validate(data); err != nil {
		return nil, err
	}
	k, err := base64.RawURLEncoding.Strict().DecodeString(key)
	if len(key) != 43 || err != nil || len(k) != 32 {
		return nil, errors.New("invalid content key")
	}
	block, _ := aes.NewCipher(k)
	aead, _ := cipher.NewGCM(block)
	plain, err := aead.Open(nil, data[12:HeaderSize], data[HeaderSize:], data[:HeaderSize])
	if err != nil {
		return nil, errors.New("cannot decrypt: wrong key or changed document")
	}
	return plain, nil
}
