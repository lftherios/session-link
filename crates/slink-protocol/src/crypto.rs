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

/// The eight points of small order, by the seven numbers a key can spell
/// their y-coordinate with; the top bit, which carries the sign of x, is
/// cleared before comparing. Zero and one are each there twice, the second
/// time with the field's size added, which every library also reads.
const SMALL_ORDER: [[u8; 32]; 7] = [
    spelt(0x00, 0x00, 0x00),
    spelt(0x01, 0x00, 0x00),
    [
        0x26, 0xe8, 0x95, 0x8f, 0xc2, 0xb2, 0x27, 0xb0, 0x45, 0xc3, 0xf4, 0x89, 0xf2, 0xef, 0x98,
        0xf0, 0xd5, 0xdf, 0xac, 0x05, 0xd3, 0xc6, 0x33, 0x39, 0xb1, 0x38, 0x02, 0x88, 0x6d, 0x53,
        0xfc, 0x05,
    ],
    [
        0xc7, 0x17, 0x6a, 0x70, 0x3d, 0x4d, 0xd8, 0x4f, 0xba, 0x3c, 0x0b, 0x76, 0x0d, 0x10, 0x67,
        0x0f, 0x2a, 0x20, 0x53, 0xfa, 0x2c, 0x39, 0xcc, 0xc6, 0x4e, 0xc7, 0xfd, 0x77, 0x92, 0xac,
        0x03, 0x7a,
    ],
    spelt(0xec, 0xff, 0x7f),
    spelt(0xed, 0xff, 0x7f),
    spelt(0xee, 0xff, 0x7f),
];

const fn spelt(first: u8, middle: u8, last: u8) -> [u8; 32] {
    let mut bytes = [middle; 32];
    bytes[0] = first;
    bytes[31] = last;
    bytes
}

/// Whether a signing key is a point of small order. Every library verifies
/// under one, and for the identity point one signature fits every message,
/// so such a key proves nothing about who signed.
pub(crate) fn small_order(public: &[u8; 32]) -> bool {
    let mut y = *public;
    y[31] &= 0x7f;
    SMALL_ORDER.contains(&y)
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
