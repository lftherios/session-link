//! This machine as a device of an account: its two private keys, and what it
//! has pinned of the account's identity. The pins are the root key, the last
//! event it saw and the highest vault revision; an answer that holds another
//! root, an older history or an older vault is refused.
//!
//! A device that sets an account up pins the identity it made before the
//! upload, and keeps the exact signed request until the service has it, so
//! a retry sends the same identity and the recovery key it was shown stays
//! the one that works.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};
use slink_local::write_private;
use slink_protocol::encoding::digest;
use slink_protocol::{
    AgreementKey, Kind, Record, Signed, SigningKey, State, Verified, device_id,
    recovery_token_matches,
};

use crate::identity::{Error, Update, refused, signed, signed_json};

/// The device's keys and pins.
#[derive(Debug)]
pub struct Device {
    pub sign: SigningKey,
    pub agreement: AgreementKey,
    /// The root key pinned, empty until the device has seen an identity.
    pub root: String,
    /// The sequence number of the last event seen, -1 before the first.
    pub seq: i64,
    pub head: String,
    pub revision: u64,
    /// The last approval request this device made.
    pub request: Option<Signed>,
    /// The recovery key of a setup made here, until its owner says it is
    /// saved.
    pub pending_recovery: String,
    /// The setup made here, until the service is seen to hold it.
    pub pending_setup: Option<Update>,
}

impl Device {
    pub fn new(seed: [u8; 32], agreement: [u8; 32]) -> Device {
        Device {
            sign: SigningKey::from_seed(seed),
            agreement: AgreementKey::from_bytes(agreement),
            root: String::new(),
            seq: -1,
            head: String::new(),
            revision: 0,
            request: None,
            pending_recovery: String::new(),
            pending_setup: None,
        }
    }

    /// The device as a history lists it, with no wrapper yet.
    pub fn public(&self, name: &str) -> slink_protocol::Device {
        let (sign_key, box_key) = (self.sign.public(), self.agreement.public());
        slink_protocol::Device {
            id: device_id(&sign_key, &box_key),
            name: name.to_owned(),
            sign_key,
            box_key,
            wrap: String::new(),
        }
    }

    pub fn id(&self) -> String {
        self.public("").id
    }

    /// A setup made here that the service never acknowledged, while the
    /// account holds an identity this device's keys are not in: another
    /// device set the account up first, or the service replaced the upload.
    /// The two look the same from here, so only discarding the setup moves
    /// past it. An identity that does list these keys can only have been
    /// built from the upload, and the pins refuse it.
    pub fn setup_conflict(&self, state: &State) -> bool {
        self.pending_setup.is_some()
            && self.root != state.root
            && state.device(&self.id()).is_none()
    }

    /// Checks `verified`, the record the service holds, against the pins,
    /// and moves them on to it.
    pub fn pin(&mut self, record: &Record, verified: &Verified) -> Result<(), Error> {
        let state = &verified.state;
        if self.setup_conflict(state) {
            return Err(Error::SetupConflict);
        }
        if !self.pending_recovery.is_empty()
            && !recovery_token_matches(&self.pending_recovery, &state.account, &state.root)
        {
            return Err(refused(
                "server recovery identity does not match the saved recovery key",
            ));
        }
        if !self.root.is_empty() && self.root != state.root {
            return Err(refused(
                "server recovery identity changed; refusing replacement",
            ));
        }
        let seen = usize::try_from(self.seq).ok();
        if seen.is_some_and(|seq| seq >= record.events.len())
            || self.revision > verified.vault.revision
        {
            return Err(refused("server returned an older encrypted vault"));
        }
        if let Some(seq) = seen.filter(|_| !self.head.is_empty())
            && record.events[seq].hash(Kind::Event)? != self.head
        {
            return Err(refused(
                "device history conflicts with this device's saved history",
            ));
        }
        self.root = state.root.clone();
        self.seq = i64::try_from(state.seq).unwrap_or(i64::MAX);
        self.head = verified.head.clone();
        self.revision = verified.vault.revision;
        self.pending_setup = None;
        Ok(())
    }

    /// As Go writes its device file.
    fn encode(&self) -> String {
        let mut fields = Map::new();
        fields.insert("seed".into(), json!(STANDARD.encode(self.sign.seed())));
        fields.insert("box".into(), json!(STANDARD.encode(self.agreement.bytes())));
        fields.insert("root".into(), json!(self.root));
        fields.insert("seq".into(), json!(self.seq));
        fields.insert("head".into(), json!(self.head));
        fields.insert("revision".into(), json!(self.revision));
        if let Some(request) = &self.request {
            fields.insert("request".into(), signed_json(request));
        }
        if !self.pending_recovery.is_empty() {
            fields.insert("pending_recovery".into(), json!(self.pending_recovery));
        }
        if let Some(update) = &self.pending_setup {
            fields.insert("pending_setup".into(), update.to_json());
        }
        slink_format::encode(&Value::Object(fields))
    }

    fn decode(raw: &[u8]) -> Option<Device> {
        let value: Value = serde_json::from_slice(raw).ok()?;
        let key = |name: &str| -> Option<[u8; 32]> {
            STANDARD.decode(value[name].as_str()?).ok()?.try_into().ok()
        };
        let text = |name: &str| match &value[name] {
            Value::Null => Some(String::new()),
            Value::String(text) => Some(text.clone()),
            _ => None,
        };
        let mut device = Device::new(key("seed")?, key("box")?);
        device.root = text("root")?;
        device.seq = value["seq"].as_i64()?;
        device.head = text("head")?;
        device.revision = value["revision"].as_u64()?;
        device.pending_recovery = text("pending_recovery")?;
        device.request = match &value["request"] {
            Value::Null => None,
            request => Some(signed(request)?),
        };
        device.pending_setup = match &value["pending_setup"] {
            Value::Null => None,
            update => Some(Update::from_json(update)?),
        };
        Some(device)
    }
}

/// Where a device's file is kept: one for each server and account.
pub fn file(home: &Path, server: &str, account: &str) -> PathBuf {
    home.join("identity").join(format!(
        "{}.json",
        digest(format!("{server}\0{account}").as_bytes())
    ))
}

pub fn load(file: &Path) -> Result<Option<Device>, Error> {
    match fs::read(file) {
        Ok(raw) => Device::decode(&raw)
            .map(Some)
            .ok_or_else(|| refused("cannot read local device keys")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(refused(&error.to_string())),
    }
}

pub fn save(file: &Path, device: &Device) -> Result<(), Error> {
    write_private(file, device.encode().as_bytes()).map_err(|error| refused(&error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_file_is_read_back_as_written() {
        let mut device = Device::new([1; 32], [2; 32]);
        device.root = "root".into();
        device.seq = 3;
        device.head = "head".into();
        device.revision = 4;
        device.pending_recovery = "slr1.a.b".into();
        device.request = Some(Signed {
            payload: "p".into(),
            signature: "s".into(),
        });
        device.pending_setup = Some(Update {
            expected_head: String::new(),
            expected_revision: 0,
            event: None,
            vault: Signed {
                payload: "v".into(),
                signature: "w".into(),
            },
        });
        let text = device.encode();
        assert!(text.contains(&format!("\"seed\":\"{}\"", STANDARD.encode([1; 32]))));
        let read = Device::decode(text.as_bytes()).unwrap();
        assert_eq!(read.encode(), text);
        assert_eq!(read.id(), device.id());
        let fresh = Device::new([1; 32], [2; 32]);
        let text = fresh.encode();
        assert!(!text.contains("pending"), "{text}");
        assert_eq!(Device::decode(text.as_bytes()).unwrap().seq, -1);
        assert!(Device::decode(br#"{"seed":"AAAA","box":"AAAA"}"#).is_none());
    }
}
