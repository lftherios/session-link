//! The three primitives checking needs, and the only file that names the
//! library they come from. ring is what TLS already links, and on signatures
//! it decides as Go's `crypto/ed25519` and OpenSSL do (`docs/rust-protocol.md`).

use ring::aead::{AES_256_GCM, Aad, LessSafeKey, Nonce, UnboundKey};
use ring::digest::{SHA256, digest};
use ring::signature::{ED25519, UnparsedPublicKey};

pub(crate) fn sha256(bytes: &[u8]) -> [u8; 32] {
    let mut out = [0; 32];
    out.copy_from_slice(digest(&SHA256, bytes).as_ref());
    out
}

pub(crate) fn ed25519_verify(public: &[u8; 32], message: &[u8], signature: &[u8; 64]) -> bool {
    UnparsedPublicKey::new(&ED25519, public)
        .verify(message, signature)
        .is_ok()
}

/// Opens `sealed`, the ciphertext followed by its 16-byte tag, in place.
/// Nothing of the plaintext is returned unless the tag holds.
pub(crate) fn aes_256_gcm_open(
    key: &[u8; 32],
    nonce: [u8; 12],
    associated: &[u8],
    mut sealed: Vec<u8>,
) -> Option<Vec<u8>> {
    let key = LessSafeKey::new(UnboundKey::new(&AES_256_GCM, key).ok()?);
    let length = key
        .open_in_place(
            Nonce::assume_unique_for_key(nonce),
            Aad::from(associated),
            &mut sealed,
        )
        .ok()?
        .len();
    sealed.truncate(length);
    Some(sealed)
}
