//! The encrypted document envelope: 24 bytes of header, then the document
//! under AES-256-GCM with the whole header as associated data.
//!
//! The header is eight bytes of magic and version, four reserved bytes that
//! are zero, and the 12-byte nonce. The key travels in a link's fragment or in
//! the vault and never in a request.

use crate::Error;
use crate::crypto::aes_256_gcm_open;
use crate::encoding::decode_exact;

pub const MAX_PLAINTEXT: usize = 25 * 1024 * 1024;
pub const HEADER_SIZE: usize = 24;
const TAG_SIZE: usize = 16;
pub const MAX_ENVELOPE: usize = HEADER_SIZE + MAX_PLAINTEXT + TAG_SIZE;

const MAGIC: [u8; 8] = *b"SLINKE\x00\x01";

/// Whether `data` has the size and header of an envelope this build reads.
/// It says nothing about whether the envelope will open.
pub fn validate(data: &[u8]) -> Result<(), Error> {
    if data.len() < HEADER_SIZE + TAG_SIZE
        || data.len() > MAX_ENVELOPE
        || data[..8] != MAGIC
        || data[8..12] != [0; 4]
    {
        return Err(Error::Envelope);
    }
    Ok(())
}

/// The document inside `data`, if `key` opens it. A changed byte anywhere,
/// header included, fails as a whole.
pub fn decrypt(data: &[u8], key: &str) -> Result<Vec<u8>, Error> {
    validate(data)?;
    let key: [u8; 32] = decode_exact(key).map_err(|_| Error::ContentKey)?;
    let (header, sealed) = data.split_at(HEADER_SIZE);
    let mut nonce = [0; 12];
    nonce.copy_from_slice(&header[12..]);
    aes_256_gcm_open(&key, nonce, header, sealed.to_vec()).ok_or(Error::Decrypt)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_size_is_bounded_on_both_sides() {
        let mut data = vec![0; MAX_ENVELOPE + 1];
        data[..8].copy_from_slice(&MAGIC);
        assert_eq!(validate(&data), Err(Error::Envelope));
        assert_eq!(validate(&data[..MAX_ENVELOPE]), Ok(()));
        assert_eq!(validate(&data[..HEADER_SIZE + TAG_SIZE]), Ok(()));
        assert_eq!(
            validate(&data[..HEADER_SIZE + TAG_SIZE - 1]),
            Err(Error::Envelope)
        );
        assert_eq!(validate(&[]), Err(Error::Envelope));
    }

    #[test]
    fn a_key_is_checked_before_anything_is_opened() {
        let mut data = vec![0; HEADER_SIZE + TAG_SIZE];
        data[..8].copy_from_slice(&MAGIC);
        let key = "A".repeat(43);
        assert_eq!(decrypt(&data, &key), Err(Error::Decrypt));
        for bad in [&key[..42], &format!("{key}A"), &format!("{key}="), ""] {
            assert_eq!(decrypt(&data, bad), Err(Error::ContentKey), "{bad:?}");
        }
        // An envelope that is not one is reported as that, whatever the key.
        assert_eq!(decrypt(&data[1..], ""), Err(Error::Envelope));
    }
}
