//! A device's request to join an account. A new device makes its two keys,
//! signs a request with the first, and shows a code; a device the account
//! already holds is given the code by its owner and, if it is the code of a
//! request it can see, wraps the vault key to the second.
//!
//! The code is twelve characters of the hash of the request as it was
//! signed. It ties what the owner saw on one screen to the keys the other
//! device is about to trust: the service that carries the request cannot
//! swap the keys and keep the code.

use crate::Error;
use crate::encoding::decode_exact;
use crate::history::MAX_DEVICE_NAME;
use crate::keys::device_id;
use crate::payload::{self, Fields, Object};
use crate::signed::{Kind, Signed};

/// How long a request can be approved for, from the time it names.
pub const REQUEST_LIFETIME_MS: u64 = 10 * 60 * 1000;
/// How far ahead of the reader's clock a request may be dated.
const CLOCK_AHEAD_MS: u64 = 60 * 1000;

/// A device asking to be approved. Keys are base64url of 32 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Request {
    pub account: String,
    /// The root key of the history the device saw.
    pub root: String,
    /// What the device calls itself, at most [`MAX_DEVICE_NAME`] bytes.
    pub name: String,
    /// The key the request is signed with: the payload's `sign`.
    pub sign_key: String,
    /// The key the vault key would be wrapped to: the payload's `box`.
    pub box_key: String,
    /// 24 random bytes, so that two requests from one device differ.
    pub nonce: String,
    /// When it was made, in milliseconds since 1970.
    pub created: u64,
}

const FIELDS: &[&str] = &["account", "root", "name", "sign", "box", "nonce", "created"];

impl Request {
    /// The id the device will have in the history.
    pub fn id(&self) -> String {
        device_id(&self.sign_key, &self.box_key)
    }

    /// The request as a payload, written as the Go CLI writes it.
    pub fn encode(&self) -> Vec<u8> {
        Object::new()
            .text("account", &self.account)
            .text("root", &self.root)
            .text("name", &self.name)
            .text("sign", &self.sign_key)
            .text("box", &self.box_key)
            .text("nonce", &self.nonce)
            .count("created", self.created)
            .finish()
            .into_bytes()
    }

    /// The request a statement holds, if it is one the service would store
    /// and the device it names signed it. Whether it is for this account, and
    /// whether it is still pending, are the caller's to check.
    pub fn verify(signed: &Signed) -> Result<Request, Error> {
        let raw = signed.read(Kind::Request)?;
        let request = read(&raw).ok_or(Error::Request)?;
        let key = decode_exact(&request.sign_key).map_err(|_| Error::Request)?;
        signed.verify_bytes(Kind::Request, &raw, &key)?;
        Ok(request)
    }

    /// The request a statement holds, without checking who signed it: only
    /// for finding the one statement that is then verified.
    pub fn read(signed: &Signed) -> Result<Request, Error> {
        read(&signed.read(Kind::Request)?).ok_or(Error::Request)
    }

    /// Whether the request can still be approved at `now`, in milliseconds
    /// since 1970: made within the last ten minutes, and not dated more than
    /// a minute ahead.
    pub fn pending(&self, now: u64) -> bool {
        self.created.saturating_add(REQUEST_LIFETIME_MS) > now
            && self.created <= now.saturating_add(CLOCK_AHEAD_MS)
    }
}

fn read(raw: &[u8]) -> Option<Request> {
    let value = payload::parse(raw)?;
    let fields = Fields::of(&value, FIELDS)?;
    let request = Request {
        account: fields.string("account")?.to_owned(),
        root: fields.string("root")?.to_owned(),
        name: fields.string("name")?.to_owned(),
        sign_key: fields.string("sign")?.to_owned(),
        box_key: fields.string("box")?.to_owned(),
        nonce: fields.string("nonce")?.to_owned(),
        created: fields.integer("created")?,
    };
    decode_exact::<32>(&request.root).ok()?;
    decode_exact::<32>(&request.box_key).ok()?;
    decode_exact::<24>(&request.nonce).ok()?;
    (request.name.len() <= MAX_DEVICE_NAME).then_some(request)
}

const BASE32: &[u8; 32] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";

/// The code a request is confirmed with: the first sixty bits of the hash of
/// its payload, in base32, as `XXXX-XXXX-XXXX`.
pub fn confirmation(signed: &Signed) -> Result<String, Error> {
    let hash: [u8; 32] = decode_exact(&signed.hash(Kind::Request)?)?;
    let bits = u64::from_be_bytes(*hash.first_chunk().expect("a hash of 32 bytes"));
    let mut code = String::with_capacity(14);
    for index in 0..12 {
        if index > 0 && index % 4 == 0 {
            code.push('-');
        }
        code.push(BASE32[(bits >> (59 - 5 * index) & 31) as usize] as char);
    }
    Ok(code)
}

/// Whether what a person typed is the code of this request. Hyphens, spaces
/// and the case of a letter are theirs to choose.
pub fn code_matches(typed: &str, signed: &Signed) -> bool {
    let plain = |text: &str| -> String {
        text.chars()
            .filter(|character| !matches!(character, '-' | ' '))
            .map(|character| character.to_ascii_uppercase())
            .collect()
    };
    confirmation(signed).is_ok_and(|code| plain(typed) == plain(&code))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::encoding::encode;
    use crate::keys::{AgreementKey, SigningKey};

    fn request(key: &SigningKey) -> Request {
        Request {
            account: "usr_a".into(),
            root: encode(&[1; 32]),
            name: "Laptop".into(),
            sign_key: key.public(),
            box_key: AgreementKey::from_bytes([2; 32]).public(),
            nonce: encode(&[3; 24]),
            created: 1_760_000_000_000,
        }
    }

    #[test]
    fn a_request_reads_back_as_it_was_signed() {
        let key = SigningKey::from_seed([5; 32]);
        let made = request(&key);
        let signed = Signed::sign(Kind::Request, &made.encode(), &key).unwrap();
        assert_eq!(Request::verify(&signed).unwrap(), made);
        assert_eq!(made.id(), device_id(&made.sign_key, &made.box_key));
        // Signed by a key other than the one it names.
        let other = SigningKey::from_seed([6; 32]);
        let forged = Signed::sign(Kind::Request, &made.encode(), &other).unwrap();
        assert_eq!(
            Request::verify(&forged),
            Err(Error::Signature(Kind::Request))
        );
        // Signed for another purpose.
        let event = Signed::sign(Kind::Event, &made.encode(), &key).unwrap();
        assert_eq!(
            Request::verify(&event),
            Err(Error::Signature(Kind::Request))
        );
    }

    #[test]
    fn a_request_the_service_would_not_store_is_refused() {
        let key = SigningKey::from_seed([5; 32]);
        let good = request(&key);
        let refused = |change: fn(&mut Request)| {
            let mut bad = good.clone();
            change(&mut bad);
            let signed = Signed::sign(Kind::Request, &bad.encode(), &key).unwrap();
            Request::verify(&signed)
        };
        assert_eq!(refused(|r| r.name = "n".repeat(81)), Err(Error::Request));
        assert!(refused(|r| r.name = "n".repeat(80)).is_ok());
        assert!(refused(|r| r.name = String::new()).is_ok());
        assert_eq!(refused(|r| r.box_key.push('A')), Err(Error::Request));
        assert_eq!(refused(|r| r.nonce = encode(&[3; 23])), Err(Error::Request));
        assert_eq!(refused(|r| r.root = "root".into()), Err(Error::Request));
        assert_eq!(refused(|r| r.created = 1 << 53), Err(Error::Request));
        // A key of small order signs nothing.
        let mut weak = good.clone();
        weak.sign_key = encode(&[0; 32]);
        let signed = Signed::sign(Kind::Request, &weak.encode(), &key).unwrap();
        assert_eq!(Request::verify(&signed), Err(Error::SigningKey));
        // Longer than the service takes, with a field no reader knows.
        let text = String::from_utf8(good.encode()).unwrap();
        let padded = text.replace('{', &format!("{{\"pad\":\"{}\",", "p".repeat(3000)));
        assert_eq!(
            Signed::sign(Kind::Request, padded.as_bytes(), &key),
            Err(Error::TooLarge)
        );
    }

    #[test]
    fn a_request_is_pending_for_ten_minutes() {
        let key = SigningKey::from_seed([5; 32]);
        let made = request(&key);
        let at = made.created;
        assert!(made.pending(at));
        assert!(made.pending(at + REQUEST_LIFETIME_MS - 1));
        assert!(!made.pending(at + REQUEST_LIFETIME_MS));
        // A clock a minute behind the device's still reads it.
        assert!(made.pending(at - CLOCK_AHEAD_MS));
        assert!(!made.pending(at - CLOCK_AHEAD_MS - 1));
        let ancient = Request { created: 0, ..made };
        assert!(!ancient.pending(u64::MAX));
    }

    #[test]
    fn the_code_is_the_hash_in_base32() {
        let key = SigningKey::from_seed([5; 32]);
        let signed = Signed::sign(Kind::Request, &request(&key).encode(), &key).unwrap();
        let code = confirmation(&signed).unwrap();
        assert_eq!(code.len(), 14);
        assert!(
            code.split('-')
                .all(|group| group.len() == 4 && group.bytes().all(|byte| BASE32.contains(&byte)))
        );
        assert!(code_matches(&code, &signed));
        assert!(code_matches(
            &code.to_lowercase().replace('-', " "),
            &signed
        ));
        assert!(code_matches(&code.replace('-', ""), &signed));
        assert!(!code_matches(&code[..13], &signed));
        assert!(!code_matches("", &signed));
        // One character of the request changes the code.
        let mut other = request(&key);
        other.name.push('!');
        let other = Signed::sign(Kind::Request, &other.encode(), &key).unwrap();
        assert_ne!(confirmation(&other).unwrap(), code);
        assert!(!code_matches(&code, &other));
    }
}
