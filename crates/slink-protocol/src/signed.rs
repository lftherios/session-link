//! A signed statement: a payload and an Ed25519 signature over
//! `slink/<kind>/v1`, a zero byte, and the payload's bytes exactly as sent.
//! The kind in what is signed keeps a signature made for one purpose from
//! being offered for another.

use crate::Error;
use crate::crypto::{ed25519_verify, small_order};
use crate::encoding::{decode, decode_exact, digest};

/// What a statement is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Event,
    Vault,
    Request,
    RecoveryCheck,
    NamedInvite,
    RecipientClaim,
    NamedGrant,
}

impl Kind {
    pub const ALL: [Kind; 7] = [
        Kind::Event,
        Kind::Vault,
        Kind::Request,
        Kind::RecoveryCheck,
        Kind::NamedInvite,
        Kind::RecipientClaim,
        Kind::NamedGrant,
    ];

    /// The name as it is signed.
    pub fn as_str(self) -> &'static str {
        match self {
            Kind::Event => "event",
            Kind::Vault => "vault",
            Kind::Request => "request",
            Kind::RecoveryCheck => "recovery-check",
            Kind::NamedInvite => "named-invite",
            Kind::RecipientClaim => "recipient-claim",
            Kind::NamedGrant => "named-grant",
        }
    }

    /// The longest payload of this kind, as text. These are the hosted
    /// service's limits, so nothing it stores is refused for its size, and
    /// nothing longer is ever decoded.
    pub fn payload_limit(self) -> usize {
        match self {
            Kind::Request => 4096,
            Kind::Event => 32 * 1024,
            Kind::Vault => 8 * 1024 * 1024,
            _ => 64 * 1024,
        }
    }
}

/// A payload and its signature, both base64url.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Signed {
    pub payload: String,
    pub signature: String,
}

impl Signed {
    /// The payload's bytes. A payload longer than `kind` allows is refused
    /// before it is decoded.
    pub fn read(&self, kind: Kind) -> Result<Vec<u8>, Error> {
        if self.payload.len() > kind.payload_limit() {
            return Err(Error::TooLarge);
        }
        decode(&self.payload)
    }

    /// The hash that names this statement: the next event's `prev`, a
    /// vault's `head`.
    pub fn hash(&self, kind: Kind) -> Result<String, Error> {
        Ok(digest(&self.read(kind)?))
    }

    /// Whether `public` signed this payload as a statement of `kind`.
    pub fn verify(&self, kind: Kind, public: &str) -> Result<(), Error> {
        let raw = self.read(kind)?;
        self.verify_bytes(kind, &raw, &decode_exact(public)?)
    }

    /// The same, for a payload already read and a key already decoded.
    pub(crate) fn verify_bytes(
        &self,
        kind: Kind,
        raw: &[u8],
        public: &[u8; 32],
    ) -> Result<(), Error> {
        if small_order(public) {
            return Err(Error::SigningKey);
        }
        let signature: [u8; 64] = decode_exact(&self.signature)?;
        let name = kind.as_str();
        let mut message = Vec::with_capacity(name.len() + raw.len() + 10);
        message.extend_from_slice(b"slink/");
        message.extend_from_slice(name.as_bytes());
        message.extend_from_slice(b"/v1\x00");
        message.extend_from_slice(raw);
        if ed25519_verify(public, &message, &signature) {
            Ok(())
        } else {
            Err(Error::Signature(kind))
        }
    }
}
