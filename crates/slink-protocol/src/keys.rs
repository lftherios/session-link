//! The two kinds of private key an account has. A device signs with an
//! Ed25519 key and is wrapped to on an X25519 key; the recovery identity is
//! one of each, and an incoming-share key is the second kind alone.
//!
//! A key is 32 bytes that arrive from the caller: this crate reads no random
//! source. Neither type prints its bytes, and both clear them when dropped.

use std::fmt;

use zeroize::Zeroize;

use crate::crypto::{ed25519_public, x25519_public};
use crate::encoding::{digest, encode};

/// An Ed25519 key, held as its seed.
pub struct SigningKey([u8; 32]);

impl SigningKey {
    pub fn from_seed(seed: [u8; 32]) -> SigningKey {
        SigningKey(seed)
    }

    pub fn seed(&self) -> &[u8; 32] {
        &self.0
    }

    /// The public key as a history writes it.
    pub fn public(&self) -> String {
        encode(&ed25519_public(&self.0))
    }
}

/// An X25519 key.
pub struct AgreementKey([u8; 32]);

impl AgreementKey {
    pub fn from_bytes(private: [u8; 32]) -> AgreementKey {
        AgreementKey(private)
    }

    pub fn bytes(&self) -> &[u8; 32] {
        &self.0
    }

    /// The public key as a history writes it.
    pub fn public(&self) -> String {
        encode(&x25519_public(&self.0))
    }
}

impl Drop for SigningKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl Drop for AgreementKey {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SigningKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "SigningKey({})", self.public())
    }
}

impl fmt::Debug for AgreementKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "AgreementKey({})", self.public())
    }
}

/// The name a history gives a device: the hash of its two public keys.
pub fn device_id(sign_key: &str, box_key: &str) -> String {
    digest(format!("{sign_key}.{box_key}").as_bytes())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_key_is_printed_by_its_public_half() {
        let sign = SigningKey::from_seed([7; 32]);
        let agree = AgreementKey::from_bytes([7; 32]);
        for printed in [format!("{sign:?}"), format!("{agree:?}")] {
            assert!(!printed.contains("7, 7"), "{printed}");
            assert!(!printed.contains(&encode(&[7; 32])), "{printed}");
        }
        assert_eq!(
            format!("{sign:?}"),
            format!("SigningKey({})", sign.public())
        );
        assert_ne!(sign.public(), agree.public());
    }

    /// RFC 8032, test 1, and RFC 7748, section 6.1: the libraries read a
    /// private key as the standards do.
    #[test]
    fn public_keys_are_the_standards() {
        let hex = |text: &str| -> [u8; 32] {
            let mut out = [0; 32];
            for (index, byte) in out.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&text[index * 2..index * 2 + 2], 16).unwrap();
            }
            out
        };
        let sign = SigningKey::from_seed(hex(
            "9d61b19deffd5a60ba844af492ec2cc44449c5697b326919703bac031cae7f60",
        ));
        assert_eq!(
            sign.public(),
            encode(&hex(
                "d75a980182b10ab7d54bfed3c964073a0ee172f3daa62325af021a68f707511a"
            ))
        );
        let agree = AgreementKey::from_bytes(hex(
            "77076d0a7318a57d3c16c17251b26645df4c2f87ebc0992ab177fba51db92c2a",
        ));
        assert_eq!(
            agree.public(),
            encode(&hex(
                "8520f0098930a754748b7ddcb43ef75a0dbf3a0d26381af4eba4a98eaa9b4e6a"
            ))
        );
    }
}
