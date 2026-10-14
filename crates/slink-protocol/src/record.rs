//! An identity record: the device history and the vault signed against its
//! head. The vault is the account's saved keys under the vault key; this
//! checks who wrote it and for which state, and leaves it sealed.

use crate::encoding::{decode, decode_exact};
use crate::history::{History, Operation, State, verify_history};
use crate::payload::{self, Fields, Object};
use crate::signed::{Kind, Signed};
use crate::wrap::{MIN_SEALED, seal, unseal};
use crate::{Error, Refusal};

/// The longest vault ciphertext, as text.
pub const MAX_VAULT_DATA: usize = 6 * 1024 * 1024;

/// The signed statement that carries the vault.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Vault {
    pub account: String,
    /// Counts writes; the service takes a write only against the revision it
    /// holds.
    pub revision: u64,
    pub head: String,
    pub epoch: u64,
    pub signer: String,
    /// The sealed vault, base64url.
    pub data: String,
}

impl Vault {
    /// The vault as a payload, written as the Go CLI writes it.
    pub fn encode(&self) -> Vec<u8> {
        Object::new()
            .text("account", &self.account)
            .count("revision", self.revision)
            .text("head", &self.head)
            .count("epoch", self.epoch)
            .text("signer", &self.signer)
            .text("data", &self.data)
            .finish()
            .into_bytes()
    }
}

/// What a vault's ciphertext is bound to: its account, root key and epoch.
fn context(state: &State) -> String {
    format!("slink/vault/v1/{}", state.context())
}

impl State {
    /// `contents` sealed under `key`, the vault key of this state: a vault
    /// statement's `data`. The nonce is the caller's, 12 random bytes.
    pub fn seal_vault(&self, key: &[u8; 32], contents: &[u8], nonce: [u8; 12]) -> String {
        seal(key, contents, &context(self), nonce)
    }
}

/// What the service holds for an account.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Record {
    pub events: Vec<Signed>,
    pub vault: Signed,
}

/// A verified record.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Verified {
    pub state: State,
    pub head: String,
    pub vault: Vault,
}

impl Verified {
    /// What the vault holds, if `key` is the vault key of the record's state.
    /// Only a record that verified has a vault to open.
    pub fn open_vault(&self, key: &[u8; 32]) -> Result<Vec<u8>, Error> {
        unseal(key, &self.vault.data, &context(&self.state)).map_err(|_| Error::VaultKey)
    }
}

/// Verifies the history, then that the vault was written for its last state
/// by someone that state lets write it.
pub fn verify_record(record: &Record, account: &str) -> Result<Verified, Error> {
    let History { state, head } = verify_history(&record.events, account)?;
    let raw = record.vault.read(Kind::Vault)?;
    let vault = read_vault(&raw).map_err(Error::Vault)?;
    let refuse = |rule| Err(Error::Vault(rule));
    if vault.account != account {
        return refuse(Refusal::Account);
    }
    if vault.head != head {
        return refuse(Refusal::Head);
    }
    if vault.epoch != state.epoch {
        return refuse(Refusal::Epoch);
    }
    if vault.revision < 1 {
        return refuse(Refusal::Revision);
    }
    if vault.data.len() > MAX_VAULT_DATA
        || decode(&vault.data).map_or(true, |sealed| sealed.len() < MIN_SEALED)
    {
        return refuse(Refusal::Data);
    }
    // The root key writes the vault only in the same breath as it enrolls a
    // device; after that the vault is a device's to write.
    let key = if vault.signer == "root" {
        if !matches!(state.kind, Operation::Init | Operation::Recover) {
            return refuse(Refusal::Writer);
        }
        &state.root
    } else {
        match state.device(&vault.signer) {
            Some(device) => &device.sign_key,
            None => return refuse(Refusal::Writer),
        }
    };
    record
        .vault
        .verify_bytes(Kind::Vault, &raw, &decode_exact(key)?)?;
    Ok(Verified { state, head, vault })
}

const VAULT_FIELDS: &[&str] = &["account", "revision", "head", "epoch", "signer", "data"];

fn read_vault(raw: &[u8]) -> Result<Vault, Refusal> {
    let value = payload::parse(raw).ok_or(Refusal::Payload)?;
    let fields = Fields::of(&value, VAULT_FIELDS).ok_or(Refusal::Payload)?;
    let text = |name| {
        fields
            .string(name)
            .map(str::to_owned)
            .ok_or(Refusal::Payload)
    };
    Ok(Vault {
        account: text("account")?,
        revision: fields.integer("revision").ok_or(Refusal::Payload)?,
        head: text("head")?,
        epoch: fields.integer("epoch").ok_or(Refusal::Payload)?,
        signer: text("signer")?,
        data: text("data")?,
    })
}
