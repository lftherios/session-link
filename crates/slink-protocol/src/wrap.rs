//! Keys under keys. A box is bytes under AES-256-GCM with a context as
//! associated data: the vault under the vault key, the recovery package under
//! the recovery secret. A wrapper is a 32-byte key in a box that only the
//! holder of one X25519 key can open: the vault key for each device and for
//! the recovery identity, a share's content key for an account.
//!
//! The context names what a box is for, so a box moved to another account,
//! another epoch or another purpose does not open.

use crate::Error;
use crate::crypto::{aes_256_gcm_open, aes_256_gcm_seal, hkdf_sha256, x25519};
use crate::encoding::{decode, decode_exact, encode};
use crate::keys::AgreementKey;

/// A nonce and a tag: the least a box holds.
pub(crate) const MIN_SEALED: usize = 12 + 16;

/// `plain` in a box under `key`: the nonce, the ciphertext and the tag,
/// base64url. The nonce is the caller's, 12 random bytes made for this one
/// box.
pub fn seal(key: &[u8; 32], plain: &[u8], context: &str, nonce: [u8; 12]) -> String {
    let mut out = nonce.to_vec();
    out.extend(aes_256_gcm_seal(key, nonce, context.as_bytes(), plain));
    encode(&out)
}

/// What is in a box, if `key` and `context` are the ones it was sealed with.
pub fn unseal(key: &[u8; 32], sealed: &str, context: &str) -> Result<Vec<u8>, Error> {
    let mut raw = decode(sealed)?;
    if raw.len() < MIN_SEALED {
        return Err(Error::Unseal);
    }
    let body = raw.split_off(12);
    let mut nonce = [0; 12];
    nonce.copy_from_slice(&raw);
    aes_256_gcm_open(key, nonce, context.as_bytes(), body).ok_or(Error::Unseal)
}

/// What the two halves of a wrapper are sealed under and bound to: the
/// context, the one-time key and the recipient's key, each as it is written.
fn info(context: &str, ephemeral: &str, recipient: &str) -> String {
    format!("slink/wrap/v1/{context}/{ephemeral}/{recipient}")
}

/// `key` wrapped to the X25519 public key `recipient`: a one-time public key,
/// a dot, and the key in a box under what the two keys agree on.
///
/// `ephemeral` and `nonce` are the caller's, random and made for this one
/// wrapper. A recipient key of small order is refused: what is agreed with
/// one is known to everyone.
pub fn wrap(
    recipient: &str,
    key: &[u8; 32],
    context: &str,
    ephemeral: &AgreementKey,
    nonce: [u8; 12],
) -> Result<String, Error> {
    let public: [u8; 32] = decode_exact(recipient)?;
    let secret = x25519(ephemeral.bytes(), &public).ok_or(Error::AgreementKey)?;
    let ephemeral = ephemeral.public();
    let info = info(context, &ephemeral, recipient);
    let derived = hkdf_sha256(secret.as_slice(), info.as_bytes());
    let sealed = seal(&derived, key, &info, nonce);
    Ok(format!("{ephemeral}.{sealed}"))
}

/// The two halves of a wrapper, if it has a wrapper's shape: 43 characters
/// of key, a dot, and 80 of a box that holds 32 bytes.
pub(crate) fn split(wrapper: &str) -> Option<(&str, [u8; 32], &str)> {
    let (ephemeral, sealed) = wrapper.split_once('.')?;
    let public = decode_exact(ephemeral).ok()?;
    decode_exact::<60>(sealed).ok()?;
    Some((ephemeral, public, sealed))
}

/// The key inside a wrapper made for `private` in this context.
pub fn unwrap(private: &AgreementKey, wrapper: &str, context: &str) -> Result<[u8; 32], Error> {
    let (ephemeral, public, sealed) = split(wrapper).ok_or(Error::Wrapper)?;
    let secret = x25519(private.bytes(), &public).ok_or(Error::Unwrap)?;
    let info = info(context, ephemeral, &private.public());
    let derived = hkdf_sha256(secret.as_slice(), info.as_bytes());
    let key = unseal(&derived, sealed, &info).map_err(|_| Error::Unwrap)?;
    key.try_into().map_err(|_| Error::Unwrap)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_box_opens_with_its_key_and_context_only() {
        let sealed = seal(&[1; 32], b"contents", "context", [2; 12]);
        assert_eq!(unseal(&[1; 32], &sealed, "context").unwrap(), b"contents");
        assert_eq!(unseal(&[3; 32], &sealed, "context"), Err(Error::Unseal));
        assert_eq!(unseal(&[1; 32], &sealed, "contexT"), Err(Error::Unseal));
        assert_eq!(unseal(&[1; 32], &sealed, ""), Err(Error::Unseal));
        // The nonce travels first, as it was given.
        assert_eq!(decode(&sealed).unwrap()[..12], [2; 12]);
        assert_eq!(decode(&sealed).unwrap().len(), 12 + 8 + 16);
    }

    #[test]
    fn a_box_of_nothing_is_a_nonce_and_a_tag() {
        let sealed = seal(&[1; 32], b"", "", [0; 12]);
        assert_eq!(decode(&sealed).unwrap().len(), MIN_SEALED);
        assert_eq!(unseal(&[1; 32], &sealed, "").unwrap(), b"");
        let short = encode(&decode(&sealed).unwrap()[..MIN_SEALED - 1]);
        assert_eq!(unseal(&[1; 32], &short, ""), Err(Error::Unseal));
        assert_eq!(unseal(&[1; 32], "", ""), Err(Error::Unseal));
        assert_eq!(
            unseal(&[1; 32], &format!("{sealed}="), ""),
            Err(Error::Encoding)
        );
    }

    #[test]
    fn a_wrapper_opens_for_its_recipient_in_its_context() {
        let recipient = AgreementKey::from_bytes([4; 32]);
        let other = AgreementKey::from_bytes([5; 32]);
        let ephemeral = AgreementKey::from_bytes([6; 32]);
        let wrapper = wrap(&recipient.public(), &[9; 32], "a/b/1", &ephemeral, [7; 12]).unwrap();
        assert_eq!(wrapper.len(), 124);
        assert_eq!(&wrapper[..43], ephemeral.public());
        assert_eq!(unwrap(&recipient, &wrapper, "a/b/1").unwrap(), [9; 32]);
        assert_eq!(unwrap(&other, &wrapper, "a/b/1"), Err(Error::Unwrap));
        assert_eq!(unwrap(&recipient, &wrapper, "a/b/2"), Err(Error::Unwrap));
        // The one-time key is a key like any other: it opens nothing.
        assert_eq!(unwrap(&ephemeral, &wrapper, "a/b/1"), Err(Error::Unwrap));
    }

    #[test]
    fn a_wrapper_has_one_shape() {
        let recipient = AgreementKey::from_bytes([4; 32]);
        let ephemeral = AgreementKey::from_bytes([6; 32]);
        let wrapper = wrap(&recipient.public(), &[9; 32], "c", &ephemeral, [7; 12]).unwrap();
        for bad in [
            wrapper[..123].to_owned(),
            format!("{wrapper}A"),
            wrapper.replace('.', "_"),
            format!("{}A.{}", &wrapper[..43], &wrapper[45..]),
            format!("{}.{}", &wrapper[..43], &wrapper[..43]),
            format!("{wrapper}="),
            String::new(),
            ".".repeat(124),
        ] {
            assert_eq!(unwrap(&recipient, &bad, "c"), Err(Error::Wrapper), "{bad}");
        }
    }

    #[test]
    fn nothing_is_wrapped_to_a_key_that_is_not_one() {
        let ephemeral = AgreementKey::from_bytes([6; 32]);
        let to = |recipient: &str| wrap(recipient, &[9; 32], "c", &ephemeral, [7; 12]);
        assert_eq!(to(&encode(&[0; 32])), Err(Error::AgreementKey));
        let mut one = [0; 32];
        one[0] = 1;
        assert_eq!(to(&encode(&one)), Err(Error::AgreementKey));
        assert_eq!(to(&encode(&[4; 31])), Err(Error::Encoding));
        assert_eq!(to(""), Err(Error::Encoding));
    }
}
