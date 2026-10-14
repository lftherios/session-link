//! The encrypted document envelope: 24 bytes of header, then the document
//! under AES-256-GCM with the whole header as associated data.
//!
//! The header is eight bytes of magic and version, four reserved bytes that
//! are zero, and the 12-byte nonce. The key travels in a link's fragment or in
//! the vault and never in a request.

use crate::Error;
use crate::crypto::{aes_256_gcm_open, aes_256_gcm_seal};
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

/// `plaintext` in an envelope under `key`. The key and the nonce are the
/// caller's: 32 and 12 random bytes made for this one document. The key
/// travels as `encoding::encode` writes it.
pub fn encrypt(plaintext: &[u8], key: &[u8; 32], nonce: [u8; 12]) -> Result<Vec<u8>, Error> {
    if plaintext.len() > MAX_PLAINTEXT {
        return Err(Error::Plaintext);
    }
    let mut data = Vec::with_capacity(HEADER_SIZE + plaintext.len() + TAG_SIZE);
    data.extend_from_slice(&MAGIC);
    data.extend_from_slice(&[0; 4]);
    data.extend_from_slice(&nonce);
    let sealed = aes_256_gcm_seal(key, nonce, &data, plaintext);
    data.extend(sealed);
    Ok(data)
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
    fn what_is_sealed_opens_and_nothing_longer_is_sealed() {
        use crate::encoding::encode;
        let data = encrypt(b"{}", &[3; 32], [5; 12]).unwrap();
        assert_eq!(data.len(), HEADER_SIZE + 2 + TAG_SIZE);
        assert_eq!(data[..8], MAGIC);
        assert_eq!(data[8..12], [0; 4]);
        assert_eq!(data[12..24], [5; 12]);
        assert_eq!(decrypt(&data, &encode(&[3; 32])).unwrap(), b"{}");
        assert_eq!(decrypt(&data, &encode(&[4; 32])), Err(Error::Decrypt));
        let empty = encrypt(b"", &[3; 32], [5; 12]).unwrap();
        assert_eq!(decrypt(&empty, &encode(&[3; 32])).unwrap(), b"");
        assert_eq!(
            encrypt(&vec![0; MAX_PLAINTEXT + 1], &[3; 32], [5; 12]),
            Err(Error::Plaintext)
        );
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
