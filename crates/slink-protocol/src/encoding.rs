//! Keys, payloads, signatures and hashes travel as base64url without padding,
//! and only in their one canonical spelling: a second spelling of the same
//! bytes would be a second name for one key or one event.

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;

use crate::Error;
use crate::crypto::sha256;

pub(crate) fn encode(bytes: &[u8]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// The bytes `text` spells. Padding, the `+/` alphabet, white space and bits
/// left over in the last character are all refused.
pub(crate) fn decode(text: &str) -> Result<Vec<u8>, Error> {
    URL_SAFE_NO_PAD.decode(text).map_err(|_| Error::Encoding)
}

/// The same, for a value of one length: a key, a signature, a hash.
pub(crate) fn decode_exact<const N: usize>(text: &str) -> Result<[u8; N], Error> {
    // Checked first, so that no hostile length is ever decoded.
    if text.len() != (N * 4).div_ceil(3) {
        return Err(Error::Encoding);
    }
    let mut out = [0; N];
    match URL_SAFE_NO_PAD.decode_slice(text, &mut out) {
        Ok(length) if length == N => Ok(out),
        _ => Err(Error::Encoding),
    }
}

/// SHA-256 as the protocol writes it: the hash of an event, the id of a
/// device.
pub(crate) fn digest(bytes: &[u8]) -> String {
    encode(&sha256(bytes))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_the_canonical_spelling_decodes() {
        assert_eq!(decode("AAEC").unwrap(), [0, 1, 2]);
        assert_eq!(decode("").unwrap(), [0u8; 0]);
        assert_eq!(decode("AA").unwrap(), [0]);
        // The same byte with bits left over, padded, in the other alphabet,
        // and with white space.
        for text in ["AB", "AA==", "+/8", "AA\n", " AA", "A"] {
            assert_eq!(decode(text), Err(Error::Encoding), "{text:?}");
        }
        assert_eq!(decode("-_8").unwrap(), [0xfb, 0xff]);
    }

    #[test]
    fn a_fixed_length_is_checked_before_decoding() {
        let key = encode(&[7; 32]);
        assert_eq!(decode_exact::<32>(&key).unwrap(), [7; 32]);
        assert!(decode_exact::<32>(&encode(&[7; 31])).is_err());
        assert!(decode_exact::<32>(&encode(&[7; 33])).is_err());
        assert!(decode_exact::<32>(&format!("{key}=")).is_err());
        // 43 characters whose last one carries two stray bits.
        let stray = format!("{}B", &key[..42]);
        assert!(decode_exact::<32>(&stray).is_err());
        assert!(decode_exact::<64>(&"A".repeat(1 << 20)).is_err());
    }
}
