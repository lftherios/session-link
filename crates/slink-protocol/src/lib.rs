//! The session.link protocol in Rust, as far as checking goes: the encrypted
//! document envelope, signed statements, an account's signed device history
//! and the vault record that hangs off it.
//!
//! Every function here is deterministic and takes its input as untrusted. It
//! returns what was verified or one [`Error`], and never part of a result.
//! Nothing here makes a key, a nonce or a signature, reads a clock or touches
//! the network or the filesystem; those arrive with the client.
//!
//! The Go CLI (`go/internal/sealed`, `go/internal/identity`), the viewer
//! (`packages/viewer`) and the hosted service each have these rules too.
//! `testdata/protocol` holds the cases all of them are held to.

mod crypto;
mod encoding;
pub mod envelope;
mod history;
mod payload;
mod record;
mod signed;

use std::fmt;

pub use history::{
    Device, History, MAX_DEVICE_NAME, MAX_DEVICES, MAX_EVENTS, Operation, State, verify_history,
};
pub use record::{MAX_VAULT_DATA, Record, Vault, Verified, verify_record};
pub use signed::{Kind, Signed};

/// Why something was not accepted. The text is what the Go CLI says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error {
    /// Not an envelope, or one in a version this build does not read.
    Envelope,
    /// The content key is not 32 bytes of canonical base64url.
    ContentKey,
    /// The envelope did not authenticate under the key.
    Decrypt,
    /// A key, payload or signature is not canonical base64url of its length.
    Encoding,
    /// A signed payload longer than its kind is allowed to be.
    TooLarge,
    /// The signature is not the key's over this payload as this kind.
    Signature(Kind),
    /// The device history broke the rule named.
    History(Refusal),
    /// The vault broke the rule named.
    Vault(Refusal),
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Envelope => f.write_str("invalid or unsupported encrypted session envelope"),
            Error::ContentKey => f.write_str("invalid content key"),
            Error::Decrypt => f.write_str("cannot decrypt: wrong key or changed document"),
            Error::Encoding => f.write_str("invalid key encoding"),
            Error::TooLarge => f.write_str("signed object exceeds the supported limit"),
            Error::Signature(kind) => write!(f, "invalid {} signature", kind.as_str()),
            Error::History(_) => f.write_str("invalid signed device history"),
            Error::Vault(_) => f.write_str("invalid signed vault"),
        }
    }
}

impl std::error::Error for Error {}

/// The rule a history or a vault broke. The Go CLI and the viewer report one
/// message for all of them; the name is here so that a case in
/// `testdata/protocol` can say which rule it is about and be held to it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Refusal {
    /// No events, or more than [`MAX_EVENTS`].
    Length,
    /// A payload or signature that is not canonical base64url, or too long.
    Signed,
    /// The payload is not a JSON object holding each field in its type.
    Payload,
    /// Signed for another account.
    Account,
    /// `seq` is not the event's place in the history.
    Sequence,
    /// `prev` is not the hash of the event before.
    Previous,
    /// `epoch` is below one, or not what the vault's history is at.
    Epoch,
    /// No device, or more than [`MAX_DEVICES`].
    Devices,
    /// A root, recovery, incoming-share, signing or agreement key that is not
    /// 32 bytes.
    Key,
    /// A key wrapper or the recovery box does not have its shape.
    Wrapper,
    /// A device whose id is not the hash of its keys, that appears twice, or
    /// whose name is longer than [`MAX_DEVICE_NAME`] bytes.
    Device,
    /// Not an operation, or `init` anywhere but first.
    Kind,
    /// The first event is not `init` by the root key with one device at
    /// epoch one.
    Initial,
    /// The root key, the recovery key or the recovery box changed.
    Recovery,
    /// The signer is not a device the state before approved, is the root key
    /// where only recovery may use it, or revoked itself.
    Signer,
    /// The signature is not the signer's.
    Signature,
    /// A device already in the history changed its name or keys, or its
    /// wrapper outside a revocation.
    DeviceChanged,
    /// The incoming-share key changed where it must stay, or stayed where it
    /// must change.
    Inbox,
    /// An incoming-share key that was retired came back.
    InboxReuse,
    /// Devices were added or removed, or the epoch or the recovery wrapper
    /// moved, in a way the operation does not allow.
    Operation,
    /// A revocation kept a wrapper made for the epoch before.
    Rotation,
    /// The vault is not signed against the head of its history.
    Head,
    /// The vault's revision is below one.
    Revision,
    /// The vault's ciphertext is too long, too short or not base64url.
    Data,
    /// The vault's signer may not write it.
    Writer,
}

impl Refusal {
    pub const ALL: [Refusal; 25] = [
        Refusal::Length,
        Refusal::Signed,
        Refusal::Payload,
        Refusal::Account,
        Refusal::Sequence,
        Refusal::Previous,
        Refusal::Epoch,
        Refusal::Devices,
        Refusal::Key,
        Refusal::Wrapper,
        Refusal::Device,
        Refusal::Kind,
        Refusal::Initial,
        Refusal::Recovery,
        Refusal::Signer,
        Refusal::Signature,
        Refusal::DeviceChanged,
        Refusal::Inbox,
        Refusal::InboxReuse,
        Refusal::Operation,
        Refusal::Rotation,
        Refusal::Head,
        Refusal::Revision,
        Refusal::Data,
        Refusal::Writer,
    ];

    /// The name `testdata/protocol` uses for the rule.
    pub fn as_str(self) -> &'static str {
        match self {
            Refusal::Length => "length",
            Refusal::Signed => "signed",
            Refusal::Payload => "payload",
            Refusal::Account => "account",
            Refusal::Sequence => "sequence",
            Refusal::Previous => "previous",
            Refusal::Epoch => "epoch",
            Refusal::Devices => "devices",
            Refusal::Key => "key",
            Refusal::Wrapper => "wrapper",
            Refusal::Device => "device",
            Refusal::Kind => "kind",
            Refusal::Initial => "initial",
            Refusal::Recovery => "recovery",
            Refusal::Signer => "signer",
            Refusal::Signature => "signature",
            Refusal::DeviceChanged => "device-changed",
            Refusal::Inbox => "inbox",
            Refusal::InboxReuse => "inbox-reuse",
            Refusal::Operation => "operation",
            Refusal::Rotation => "rotation",
            Refusal::Head => "head",
            Refusal::Revision => "revision",
            Refusal::Data => "data",
            Refusal::Writer => "writer",
        }
    }
}
