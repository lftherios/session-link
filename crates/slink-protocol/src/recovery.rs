//! The recovery identity: the Ed25519 key that signs an account's first event
//! and any later recovery, and the X25519 key the vault key is always wrapped
//! to. Both private halves sit in the history itself, in a box under a secret
//! that only the person holds, as the recovery key they were shown once.
//!
//! The recovery key is `slr1`, a fingerprint of the account and its root key,
//! and the secret. The fingerprint lets a client say that a key is for
//! another account before it tries to open anything with it.

use zeroize::Zeroizing;

use crate::Error;
use crate::encoding::{decode_exact, digest, encode};
use crate::history::State;
use crate::keys::{AgreementKey, SigningKey};
use crate::wrap::{seal, unseal};

const VERSION: &str = "slr1";

/// The two private keys of a recovery identity.
#[derive(Debug)]
pub struct Recovery {
    /// Signs the first event and every recovery: the history's `root`.
    pub root: SigningKey,
    /// Opens the recovery wrapper: the history's `recovery`.
    pub agreement: AgreementKey,
}

fn fingerprint(account: &str, root: &str) -> String {
    digest(format!("{account}\0{root}").as_bytes())
}

fn context(account: &str, root: &str) -> String {
    format!("slink/recovery/v1/{account}/{root}")
}

/// The fingerprint and the secret of a recovery key, if it is written as one.
fn parts(token: &str) -> Option<(&str, &str)> {
    let mut parts = token.split('.');
    let found = (parts.next()?, parts.next()?, parts.next()?);
    match (found, parts.next()) {
        ((VERSION, fingerprint, secret), None) => Some((fingerprint, secret)),
        _ => None,
    }
}

/// The recovery key a person is shown for `secret`, 32 random bytes.
pub fn recovery_token(account: &str, root: &str, secret: &[u8; 32]) -> String {
    format!(
        "{VERSION}.{}.{}",
        fingerprint(account, root),
        encode(secret)
    )
}

/// Whether a recovery key was made for this account and root key. It says
/// nothing about whether the key opens the package.
pub fn recovery_token_matches(token: &str, account: &str, root: &str) -> bool {
    parts(token).is_some_and(|(found, _)| found == fingerprint(account, root))
}

impl Recovery {
    /// The recovery package for `account`: a history's `recovery_box`. The
    /// nonce is the caller's, 12 random bytes.
    pub fn seal(&self, account: &str, secret: &[u8; 32], nonce: [u8; 12]) -> String {
        let mut package = Zeroizing::new([0; 64]);
        package[..32].copy_from_slice(self.root.seed());
        package[32..].copy_from_slice(self.agreement.bytes());
        seal(
            secret,
            package.as_slice(),
            &context(account, &self.root.public()),
            nonce,
        )
    }
}

/// The recovery identity of `state`, opened with the recovery key as a person
/// typed it. The keys that come out are the ones the history names, or
/// nothing comes out.
pub fn open_recovery(token: &str, state: &State) -> Result<Recovery, Error> {
    let (found, secret) = parts(token.trim()).ok_or(Error::RecoveryToken)?;
    if found != fingerprint(&state.account, &state.root) {
        return Err(Error::RecoveryToken);
    }
    let secret = Zeroizing::new(decode_exact::<32>(secret)?);
    let package = unseal(
        &secret,
        &state.recovery_box,
        &context(&state.account, &state.root),
    )
    .map(Zeroizing::new)
    .map_err(|_| Error::RecoveryKey)?;
    let (Some(root), Some(agreement)) = (
        package.first_chunk::<32>(),
        package.last_chunk::<32>().filter(|_| package.len() == 64),
    ) else {
        return Err(Error::RecoveryKey);
    };
    let recovery = Recovery {
        root: SigningKey::from_seed(*root),
        agreement: AgreementKey::from_bytes(*agreement),
    };
    if recovery.root.public() != state.root || recovery.agreement.public() != state.recovery {
        return Err(Error::RecoveryIdentity);
    }
    Ok(recovery)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::Operation;

    fn identity() -> (Recovery, State, String) {
        let recovery = Recovery {
            root: SigningKey::from_seed([1; 32]),
            agreement: AgreementKey::from_bytes([2; 32]),
        };
        let state = State {
            account: "usr_a".into(),
            seq: 0,
            prev: String::new(),
            kind: Operation::Init,
            signer: "root".into(),
            root: recovery.root.public(),
            recovery: recovery.agreement.public(),
            recovery_box: recovery.seal("usr_a", &[3; 32], [4; 12]),
            recovery_wrap: String::new(),
            epoch: 1,
            devices: Vec::new(),
            inbox: None,
        };
        let token = recovery_token("usr_a", &state.root, &[3; 32]);
        (recovery, state, token)
    }

    #[test]
    fn the_key_a_person_holds_opens_the_package() {
        let (recovery, state, token) = identity();
        assert_eq!(state.recovery_box.len(), 123);
        assert_eq!(token.len(), 4 + 1 + 43 + 1 + 43);
        let opened = open_recovery(&format!("  {token}\n"), &state).unwrap();
        assert_eq!(opened.root.seed(), recovery.root.seed());
        assert_eq!(opened.agreement.bytes(), recovery.agreement.bytes());
        assert!(recovery_token_matches(&token, "usr_a", &state.root));
    }

    #[test]
    fn a_key_for_another_identity_is_told_from_a_wrong_one() {
        let (_, state, token) = identity();
        let other = recovery_token("usr_b", &state.root, &[3; 32]);
        assert_eq!(
            open_recovery(&other, &state).unwrap_err(),
            Error::RecoveryToken
        );
        assert!(!recovery_token_matches(&other, "usr_a", &state.root));
        for bad in [
            "",
            "slr1",
            &token[5..],
            &token.replace("slr1", "slr2"),
            &format!("{token}.x"),
            &token.replace('.', " "),
        ] {
            assert_eq!(
                open_recovery(bad, &state).unwrap_err(),
                Error::RecoveryToken,
                "{bad}"
            );
            assert!(!recovery_token_matches(bad, "usr_a", &state.root), "{bad}");
        }
        // The right fingerprint and the wrong secret.
        let wrong = recovery_token("usr_a", &state.root, &[9; 32]);
        assert!(recovery_token_matches(&wrong, "usr_a", &state.root));
        assert_eq!(
            open_recovery(&wrong, &state).unwrap_err(),
            Error::RecoveryKey
        );
        // A secret that is not 32 bytes of base64url.
        assert_eq!(
            open_recovery(&format!("{token}A"), &state).unwrap_err(),
            Error::Encoding
        );
        // A stored key is compared as it was stored.
        assert!(!recovery_token_matches(
            &format!(" {token}"),
            "usr_a",
            &state.root
        ));
    }

    #[test]
    fn a_package_must_hold_the_keys_the_history_names() {
        let (recovery, state, token) = identity();
        // The package is bound to the account: sealed for another, it does
        // not open here.
        let moved = State {
            recovery_box: recovery.seal("usr_b", &[3; 32], [4; 12]),
            ..state.clone()
        };
        assert_eq!(
            open_recovery(&token, &moved).unwrap_err(),
            Error::RecoveryKey
        );
        // A package that opens and is not two keys.
        let short = State {
            recovery_box: seal(&[3; 32], &[0; 63], &context("usr_a", &state.root), [4; 12]),
            ..state.clone()
        };
        assert_eq!(
            open_recovery(&token, &short).unwrap_err(),
            Error::RecoveryKey
        );
        // A package that holds another agreement key than the history names.
        let other = Recovery {
            root: SigningKey::from_seed([1; 32]),
            agreement: AgreementKey::from_bytes([8; 32]),
        };
        let swapped = State {
            recovery_box: other.seal("usr_a", &[3; 32], [4; 12]),
            ..state
        };
        assert_eq!(
            open_recovery(&token, &swapped).unwrap_err(),
            Error::RecoveryIdentity
        );
    }
}
