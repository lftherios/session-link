//! A share for named people. Its key is never in a link: the sender wraps it
//! to each account that may read it, in three signed statements.
//!
//! The sender invites one email address with a statement that carries a
//! commitment to a secret; the secret itself goes to that person in a link
//! the sender delivers. The recipient answers with a claim, signed by one of
//! their devices and bound to the invitation by a proof made with the secret.
//! The sender checks the claim against the recipient's own history and then
//! grants: the share's key, wrapped to the recipient's incoming-share key.
//!
//! The proof is what keeps the service out of it. The service carries all
//! three statements and could offer the sender an account of its own; it
//! cannot make a proof for a secret it never saw.

use crate::crypto::{hmac_sha256, hmac_sha256_verify};
use crate::encoding::{decode_exact, digest, encode};
use crate::history::{State, verify_history};
use crate::keys::AgreementKey;
use crate::payload::{self, Fields, Object};
use crate::signed::{Kind, Signed};
use crate::{Error, wrap};

/// What the proof is made over, before the claim's payload.
const BINDING: &[u8] = b"slink/recipient-binding/v1\x00";

/// What a share's key is wrapped under for one account: the ciphertext it
/// opens, and the account, root key and epoch of the incoming-share key.
pub fn named_context(sha256: &str, account: &str, root: &str, epoch: u64) -> String {
    format!("named/{sha256}/{account}/{root}/{epoch}")
}

/// What an invitation says of its secret, 32 random bytes, without telling
/// it.
pub fn commitment(secret: &[u8; 32]) -> String {
    digest(secret)
}

/// The proof a recipient sends with a claim: that whoever signed it held the
/// invitation's secret.
pub fn binding_proof(secret: &[u8; 32], claim: &Signed) -> Result<String, Error> {
    let raw = claim.read(Kind::RecipientClaim)?;
    Ok(encode(&hmac_sha256(secret, &[BINDING, &raw].concat())))
}

/// One person a share is for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invitation {
    /// 24 random bytes, base64url.
    pub id: String,
    pub email: String,
    /// [`commitment`] of the invitation's secret.
    pub commitment: String,
}

/// The sender's statement to one recipient. Each recipient is sent only
/// their own, so none learns who else was invited.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Invite {
    pub account: String,
    pub root: String,
    /// The head of the sender's history when this was signed.
    pub head: String,
    /// The sender's device.
    pub signer: String,
    /// The ciphertext's SHA-256, in hex.
    pub sha256: String,
    /// The sender's incoming-share key, and the epoch it is of.
    pub inbox: String,
    pub epoch: u64,
    /// The share's key wrapped to that key, for the sender's own devices.
    pub wrap: String,
    pub invitation: Invitation,
}

/// A recipient's answer to an invitation: which account and which
/// incoming-share key the share's key should be wrapped to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Claim {
    pub account: String,
    pub root: String,
    /// The head of the recipient's history when this was signed.
    pub head: String,
    /// The recipient's device.
    pub signer: String,
    pub share_id: String,
    pub invite_id: String,
    pub sha256: String,
    /// The sender's account and root key.
    pub owner: String,
    pub owner_root: String,
    /// The recipient's incoming-share key, and the epoch it is of.
    pub inbox: String,
    pub epoch: u64,
}

/// The sender's answer to a claim: the share's key, wrapped for the
/// recipient.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Grant {
    pub account: String,
    pub root: String,
    pub head: String,
    pub signer: String,
    pub share_id: String,
    pub invite_id: String,
    pub sha256: String,
    pub recipient: String,
    pub recipient_root: String,
    pub inbox: String,
    pub epoch: u64,
    pub wrap: String,
    /// The hash of the claim this answers.
    pub claim_hash: String,
}

impl Invite {
    /// The invitation as a payload, written as the Go CLI writes it.
    pub fn encode(&self) -> Vec<u8> {
        let invitation = Object::new()
            .text("id", &self.invitation.id)
            .text("email", &self.invitation.email)
            .text("commitment", &self.invitation.commitment)
            .finish();
        Object::new()
            .text("account", &self.account)
            .text("root", &self.root)
            .text("head", &self.head)
            .text("signer", &self.signer)
            .text("sha256", &self.sha256)
            .text("inbox", &self.inbox)
            .count("epoch", self.epoch)
            .text("wrap", &self.wrap)
            .json("invitation", &invitation)
            .finish()
            .into_bytes()
    }
}

impl Claim {
    /// The claim as a payload, written in the order the viewer writes it.
    pub fn encode(&self) -> Vec<u8> {
        Object::new()
            .text("account", &self.account)
            .text("root", &self.root)
            .text("head", &self.head)
            .text("signer", &self.signer)
            .text("share_id", &self.share_id)
            .text("invite_id", &self.invite_id)
            .text("sha256", &self.sha256)
            .text("owner", &self.owner)
            .text("owner_root", &self.owner_root)
            .text("inbox", &self.inbox)
            .count("epoch", self.epoch)
            .finish()
            .into_bytes()
    }
}

impl Grant {
    /// The grant as a payload, written as the Go CLI writes it.
    pub fn encode(&self) -> Vec<u8> {
        Object::new()
            .text("account", &self.account)
            .text("root", &self.root)
            .text("head", &self.head)
            .text("signer", &self.signer)
            .text("share_id", &self.share_id)
            .text("invite_id", &self.invite_id)
            .text("sha256", &self.sha256)
            .text("recipient", &self.recipient)
            .text("recipient_root", &self.recipient_root)
            .text("inbox", &self.inbox)
            .count("epoch", self.epoch)
            .text("wrap", &self.wrap)
            .text("claim_hash", &self.claim_hash)
            .finish()
            .into_bytes()
    }
}

const CLAIM_FIELDS: &[&str] = &[
    "account",
    "root",
    "head",
    "signer",
    "share_id",
    "invite_id",
    "sha256",
    "owner",
    "owner_root",
    "inbox",
    "epoch",
];

fn read_claim(raw: &[u8]) -> Option<Claim> {
    let value = payload::parse(raw)?;
    let fields = Fields::of(&value, CLAIM_FIELDS)?;
    let text = |name| fields.string(name).map(str::to_owned);
    Some(Claim {
        account: text("account")?,
        root: text("root")?,
        head: text("head")?,
        signer: text("signer")?,
        share_id: text("share_id")?,
        invite_id: text("invite_id")?,
        sha256: text("sha256")?,
        owner: text("owner")?,
        owner_root: text("owner_root")?,
        inbox: text("inbox")?,
        epoch: fields.integer("epoch")?,
    })
}

impl State {
    /// A share's key wrapped to this account's incoming-share key: the
    /// sender's own wrapper in an invitation, and a recipient's in a grant.
    /// The one-time key and the nonce are the caller's.
    pub fn wrap_share_key(
        &self,
        sha256: &str,
        key: &[u8; 32],
        ephemeral: &AgreementKey,
        nonce: [u8; 12],
    ) -> Result<String, Error> {
        let inbox = self.inbox.as_deref().ok_or(Error::Inbox)?;
        let context = named_context(sha256, &self.account, &self.root, self.epoch);
        wrap::wrap(inbox, key, &context, ephemeral, nonce)
    }
}

/// What a sender holds of the invitation a claim answers. Printed for
/// debugging, it leaves the secret out.
#[derive(Clone, Copy)]
pub struct Offer<'a> {
    pub share_id: &'a str,
    pub invite_id: &'a str,
    /// The ciphertext's SHA-256, in hex.
    pub sha256: &'a str,
    /// The sender's account and root key.
    pub owner: &'a str,
    pub owner_root: &'a str,
    /// The invitation's secret, which only the invited person was sent.
    pub secret: &'a [u8; 32],
}

impl std::fmt::Debug for Offer<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Offer")
            .field("share_id", &self.share_id)
            .field("invite_id", &self.invite_id)
            .field("sha256", &self.sha256)
            .field("owner", &self.owner)
            .field("owner_root", &self.owner_root)
            .finish_non_exhaustive()
    }
}

/// A claim the sender may answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Accepted {
    pub claim: Claim,
    /// The hash that names the claim: a grant's `claim_hash`.
    pub claim_hash: String,
    /// The recipient's account as its history stands now. The grant is
    /// wrapped to this state's incoming-share key, which a rotation since
    /// the claim may have made another than the one the claim names.
    pub recipient: State,
}

/// Whether a claim is the invited person's, made by a device their history
/// approved, for this invitation and no other. `history` is the recipient's
/// whole device history as the service gives it.
///
/// Nothing is released before this returns `Ok`.
pub fn verify_claim(
    claim: &Signed,
    proof: &str,
    offer: &Offer<'_>,
    history: &[Signed],
) -> Result<Accepted, Error> {
    let raw = claim.read(Kind::RecipientClaim).map_err(|_| Error::Claim)?;
    let binding = read_claim(&raw).ok_or(Error::Claim)?;
    let proven = decode_exact::<32>(proof)
        .is_ok_and(|tag| hmac_sha256_verify(offer.secret, &[BINDING, &raw].concat(), &tag));
    if !proven
        || binding.invite_id != offer.invite_id
        || binding.share_id != offer.share_id
        || binding.sha256 != offer.sha256
        || binding.owner != offer.owner
        || binding.owner_root != offer.owner_root
    {
        return Err(Error::ClaimProof);
    }
    let now = verify_history(history, &binding.account)?.state;
    if now.root != binding.root || now.inbox.is_none() {
        return Err(Error::RecipientIdentity);
    }
    // The claim was signed at one point of that history, by a device the
    // history held then and for the key it had then.
    let mut at = None;
    for (index, event) in history.iter().enumerate() {
        if event.hash(Kind::Event)? == binding.head {
            at = Some(index);
            break;
        }
    }
    let at = at.ok_or(Error::RecipientHistory)?;
    let then = verify_history(&history[..=at], &binding.account)?.state;
    let signed = then.device(&binding.signer).is_some_and(|device| {
        decode_exact(&device.sign_key)
            .is_ok_and(|key| claim.verify_bytes(Kind::RecipientClaim, &raw, &key).is_ok())
    });
    if !signed || then.inbox.as_deref() != Some(&binding.inbox) || then.epoch != binding.epoch {
        return Err(Error::ClaimSigner);
    }
    Ok(Accepted {
        claim_hash: digest(&raw),
        claim: binding,
        recipient: now,
    })
}

impl Accepted {
    /// The grant that answers the claim: `key`, the share's key, wrapped to
    /// the recipient's incoming-share key as it is now. `head` and `signer`
    /// are the sender's history head and device; the one-time key and the
    /// nonce are the caller's.
    pub fn grant(
        &self,
        offer: &Offer<'_>,
        head: &str,
        signer: &str,
        key: &[u8; 32],
        ephemeral: &AgreementKey,
        nonce: [u8; 12],
    ) -> Result<Grant, Error> {
        let recipient = &self.recipient;
        Ok(Grant {
            account: offer.owner.to_owned(),
            root: offer.owner_root.to_owned(),
            head: head.to_owned(),
            signer: signer.to_owned(),
            share_id: offer.share_id.to_owned(),
            invite_id: offer.invite_id.to_owned(),
            sha256: offer.sha256.to_owned(),
            recipient: recipient.account.clone(),
            recipient_root: recipient.root.clone(),
            inbox: recipient.inbox.clone().ok_or(Error::Inbox)?,
            epoch: recipient.epoch,
            wrap: recipient.wrap_share_key(offer.sha256, key, ephemeral, nonce)?,
            claim_hash: self.claim_hash.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_context_names_the_ciphertext_and_the_key_it_is_for() {
        assert_eq!(
            named_context("ab", "usr_a", "root", 3),
            "named/ab/usr_a/root/3"
        );
        assert_eq!(commitment(&[0; 32]), digest(&[0; 32]));
        assert_ne!(commitment(&[0; 32]), commitment(&[1; 32]));
    }

    #[test]
    fn a_claim_is_read_field_by_field_or_not_at_all() {
        let claim = Claim {
            account: "usr_b".into(),
            root: "root".into(),
            head: "head".into(),
            signer: "device".into(),
            share_id: "share".into(),
            invite_id: "invite".into(),
            sha256: "ab".into(),
            owner: "usr_a".into(),
            owner_root: "owner root".into(),
            inbox: "inbox".into(),
            epoch: 2,
        };
        let raw = claim.encode();
        assert_eq!(read_claim(&raw), Some(claim.clone()));
        let text = String::from_utf8(raw).unwrap();
        for bad in [
            text.replace("\"epoch\":2", "\"epoch\":\"2\""),
            text.replace("\"epoch\":2", "\"epoch\":2.0"),
            text.replace(",\"epoch\":2", ""),
            text.replace("\"owner\":", "\"Owner\":"),
            text.replace("{", "{\"epoch\":1,"),
            format!("[{text}]"),
            String::new(),
        ] {
            assert_eq!(read_claim(bad.as_bytes()), None, "{bad}");
        }
        // A field no reader knows is carried and ignored.
        let more = text.replace('{', "{\"note\":[1,null],");
        assert_eq!(read_claim(more.as_bytes()), Some(claim));
    }

    #[test]
    fn an_account_that_does_not_share_has_nothing_to_wrap_to() {
        use crate::history::Operation;
        let state = State {
            account: "usr_a".into(),
            seq: 0,
            prev: String::new(),
            kind: Operation::Init,
            signer: "root".into(),
            root: "root".into(),
            recovery: String::new(),
            recovery_box: String::new(),
            recovery_wrap: String::new(),
            epoch: 1,
            devices: Vec::new(),
            inbox: None,
        };
        let ephemeral = AgreementKey::from_bytes([6; 32]);
        assert_eq!(
            state.wrap_share_key("ab", &[9; 32], &ephemeral, [7; 12]),
            Err(Error::Inbox)
        );
        let inbox = AgreementKey::from_bytes([4; 32]);
        let sharing = State {
            inbox: Some(inbox.public()),
            ..state
        };
        let wrapper = sharing
            .wrap_share_key("ab", &[9; 32], &ephemeral, [7; 12])
            .unwrap();
        let context = named_context("ab", "usr_a", "root", 1);
        assert_eq!(wrap::unwrap(&inbox, &wrapper, &context).unwrap(), [9; 32]);
        // Bound to the ciphertext and to the epoch.
        for other in [
            named_context("ac", "usr_a", "root", 1),
            named_context("ab", "usr_a", "root", 2),
            sharing.context(),
        ] {
            assert_eq!(
                wrap::unwrap(&inbox, &wrapper, &other),
                Err(Error::Unwrap),
                "{other}"
            );
        }
    }
}
