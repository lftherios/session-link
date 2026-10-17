//! An account's signed device history: which devices may read the vault and
//! act for the account, as a chain of statements that starts at the recovery
//! key and that each later device signs in turn.
//!
//! Every event carries the whole state after it. Verification walks the chain
//! from the first event and checks each state against the one before, so what
//! comes out is the state the last event leaves and the hash that names it.

use std::collections::HashSet;

use serde_json::Value;

use crate::encoding::{decode_exact, digest};
use crate::keys::{AgreementKey, device_id};
use crate::payload::{self, Fields, Object, list};
use crate::signed::{Kind, Signed};
use crate::{Error, Refusal, wrap};

pub const MAX_EVENTS: usize = 512;
pub const MAX_DEVICES: usize = 32;
/// In bytes of UTF-8.
pub const MAX_DEVICE_NAME: usize = 80;

/// What an event does to the state before it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Operation {
    /// The first event: the recovery key enrolls the first device.
    Init,
    /// A device enrolls another.
    Approve,
    /// The recovery key enrolls a device.
    Recover,
    /// A device removes another and every key is wrapped again.
    Revoke,
    /// A device adds the account's first incoming-share key.
    Sharing,
}

impl Operation {
    pub fn as_str(self) -> &'static str {
        match self {
            Operation::Init => "init",
            Operation::Approve => "approve",
            Operation::Recover => "recover",
            Operation::Revoke => "revoke",
            Operation::Sharing => "sharing",
        }
    }
}

/// A device in a state. Keys are base64url of 32 bytes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    /// The hash of the two keys.
    pub id: String,
    pub name: String,
    /// The Ed25519 key the device signs with: the payload's `sign`.
    pub sign_key: String,
    /// The X25519 key the vault key is wrapped to: the payload's `box`.
    pub box_key: String,
    /// The vault key, wrapped to `box_key` for the state's epoch.
    pub wrap: String,
}

/// The state an event leaves the account in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct State {
    pub account: String,
    pub seq: u64,
    pub prev: String,
    pub kind: Operation,
    /// A device's id, or `root` for the recovery key.
    pub signer: String,
    pub root: String,
    pub recovery: String,
    pub recovery_box: String,
    pub recovery_wrap: String,
    pub epoch: u64,
    pub devices: Vec<Device>,
    /// The key named shares are wrapped to, once sharing is on.
    pub inbox: Option<String>,
}

impl State {
    pub fn device(&self, id: &str) -> Option<&Device> {
        self.devices.iter().find(|device| device.id == id)
    }

    /// What the wrappers and the vault of this state are bound to: the
    /// account, its root key and the epoch. A wrapper made for one epoch
    /// does not open in the next.
    pub(crate) fn context(&self) -> String {
        format!("{}/{}/{}", self.account, self.root, self.epoch)
    }

    /// The state as an event's payload, written as the Go CLI writes it:
    /// the incoming-share key first when there is one, then the other fields
    /// in the order Go declares them.
    pub fn encode(&self) -> Vec<u8> {
        let devices = self.devices.iter().map(|device| {
            Object::new()
                .text("id", &device.id)
                .text("name", &device.name)
                .text("sign", &device.sign_key)
                .text("box", &device.box_key)
                .text("wrap", &device.wrap)
                .finish()
        });
        let object = match &self.inbox {
            Some(inbox) => Object::new().text("inbox", inbox),
            None => Object::new(),
        };
        object
            .text("account", &self.account)
            .count("seq", self.seq)
            .text("prev", &self.prev)
            .text("kind", self.kind.as_str())
            .text("signer", &self.signer)
            .text("root", &self.root)
            .text("recovery", &self.recovery)
            .text("recovery_box", &self.recovery_box)
            .text("recovery_wrap", &self.recovery_wrap)
            .count("epoch", self.epoch)
            .json("devices", &list(devices))
            .finish()
            .into_bytes()
    }

    /// `key`, the vault key of this state, wrapped to an X25519 public key:
    /// a device's `wrap`, or the state's `recovery_wrap`. The one-time key
    /// and the nonce are the caller's, random and made for this wrapper.
    pub fn wrap_key(
        &self,
        recipient: &str,
        key: &[u8; 32],
        ephemeral: &AgreementKey,
        nonce: [u8; 12],
    ) -> Result<String, Error> {
        wrap::wrap(recipient, key, &self.context(), ephemeral, nonce)
    }

    /// The vault key of this state, from a wrapper it holds for `private`.
    pub fn unwrap_key(&self, private: &AgreementKey, wrapper: &str) -> Result<[u8; 32], Error> {
        wrap::unwrap(private, wrapper, &self.context())
    }
}

/// A verified history: its last state and the hash of its last event.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    pub state: State,
    pub head: String,
}

/// Verifies `events` as the whole history of `account`, from its first event.
pub fn verify_history(events: &[Signed], account: &str) -> Result<History, Error> {
    walk(events, account).map_err(Error::History)
}

fn walk(events: &[Signed], account: &str) -> Result<History, Refusal> {
    if events.is_empty() || events.len() > MAX_EVENTS {
        return Err(Refusal::Length);
    }
    let mut previous: Option<State> = None;
    let mut head = String::new();
    let mut inboxes: HashSet<String> = HashSet::new();
    for (index, event) in events.iter().enumerate() {
        let raw = event.read(Kind::Event).map_err(|_| Refusal::Signed)?;
        let next = read_state(&raw)?;
        if next.account != account {
            return Err(Refusal::Account);
        }
        if next.seq != index as u64 {
            return Err(Refusal::Sequence);
        }
        if next.prev != head {
            return Err(Refusal::Previous);
        }
        if next.epoch < 1 {
            return Err(Refusal::Epoch);
        }
        check_keys(&next)?;
        let signed_by = |key: &str| -> Result<(), Refusal> {
            let key = decode_exact(key).map_err(|_| Refusal::Key)?;
            event
                .verify_bytes(Kind::Event, &raw, &key)
                .map_err(|error| match error {
                    Error::Signature(_) => Refusal::Signature,
                    Error::SigningKey => Refusal::Key,
                    _ => Refusal::Signed,
                })
        };
        match &previous {
            None => {
                if next.kind != Operation::Init
                    || next.signer != "root"
                    || next.epoch != 1
                    || next.devices.len() != 1
                {
                    return Err(Refusal::Initial);
                }
                signed_by(&next.root)?;
            }
            Some(previous) => {
                if next.root != previous.root
                    || next.recovery != previous.recovery
                    || next.recovery_box != previous.recovery_box
                {
                    return Err(Refusal::Recovery);
                }
                // Only recovery is the root key's to sign; everything else is
                // signed by a device the state before already holds.
                let key = if next.kind == Operation::Recover {
                    if next.signer != "root" {
                        return Err(Refusal::Signer);
                    }
                    &previous.root
                } else {
                    &previous
                        .device(&next.signer)
                        .ok_or(Refusal::Signer)?
                        .sign_key
                };
                signed_by(key)?;
                step(previous, &next)?;
            }
        }
        // Every incoming-share key is introduced once; a retired key never
        // returns.
        let before = previous.as_ref().and_then(|state| state.inbox.as_ref());
        if let Some(inbox) = &next.inbox
            && Some(inbox) != before
            && !inboxes.insert(inbox.clone())
        {
            return Err(Refusal::InboxReuse);
        }
        head = digest(&raw);
        previous = Some(next);
    }
    let state = previous.ok_or(Refusal::Length)?;
    Ok(History { state, head })
}

/// Whether `next` is a state its operation may make of `previous`. Who
/// signed it is already settled.
fn step(previous: &State, next: &State) -> Result<(), Refusal> {
    let revoking = next.kind == Operation::Revoke;
    let mut added = 0;
    for device in &next.devices {
        let Some(old) = previous.device(&device.id) else {
            added += 1;
            continue;
        };
        if device.name != old.name
            || device.sign_key != old.sign_key
            || device.box_key != old.box_key
            || (!revoking && device.wrap != old.wrap)
        {
            return Err(Refusal::DeviceChanged);
        }
    }
    let removed = previous
        .devices
        .iter()
        .filter(|device| next.device(&device.id).is_none())
        .count();
    let same_epoch = next.epoch == previous.epoch;
    let same_recovery_wrap = next.recovery_wrap == previous.recovery_wrap;
    match next.kind {
        Operation::Init => Err(Refusal::Kind),
        Operation::Sharing => {
            if previous.inbox.is_some() || next.inbox.is_none() {
                return Err(Refusal::Inbox);
            }
            if added != 0 || removed != 0 || !same_epoch || !same_recovery_wrap {
                return Err(Refusal::Operation);
            }
            Ok(())
        }
        Operation::Approve | Operation::Recover => {
            if next.inbox != previous.inbox {
                return Err(Refusal::Inbox);
            }
            if added != 1 || removed != 0 || !same_epoch || !same_recovery_wrap {
                return Err(Refusal::Operation);
            }
            Ok(())
        }
        Operation::Revoke => {
            // An account that shares gets a new incoming-share key with every
            // revocation; one that does not cannot gain one this way.
            let rotated = match (&previous.inbox, &next.inbox) {
                (None, None) => true,
                (Some(old), Some(new)) => old != new,
                _ => false,
            };
            if !rotated {
                return Err(Refusal::Inbox);
            }
            if added != 0 || removed != 1 || next.epoch != previous.epoch + 1 {
                return Err(Refusal::Operation);
            }
            // The removed device knew the old vault key, so every wrapper of
            // it is replaced, the recovery key's included.
            let kept = |device: &Device| {
                previous
                    .device(&device.id)
                    .is_some_and(|old| old.wrap == device.wrap)
            };
            if same_recovery_wrap || next.devices.iter().any(kept) {
                return Err(Refusal::Rotation);
            }
            if next.device(&next.signer).is_none() {
                return Err(Refusal::Signer);
            }
            Ok(())
        }
    }
}

const STATE_FIELDS: &[&str] = &[
    "inbox",
    "account",
    "seq",
    "prev",
    "kind",
    "signer",
    "root",
    "recovery",
    "recovery_box",
    "recovery_wrap",
    "epoch",
    "devices",
];
const DEVICE_FIELDS: &[&str] = &["id", "name", "sign", "box", "wrap"];

fn read_state(raw: &[u8]) -> Result<State, Refusal> {
    let value = payload::parse(raw).ok_or(Refusal::Payload)?;
    let fields = Fields::of(&value, STATE_FIELDS).ok_or(Refusal::Payload)?;
    let text = |name| {
        fields
            .string(name)
            .map(str::to_owned)
            .ok_or(Refusal::Payload)
    };
    let devices = fields.array("devices").ok_or(Refusal::Payload)?;
    if devices.is_empty() || devices.len() > MAX_DEVICES {
        return Err(Refusal::Devices);
    }
    let kind = match fields.string("kind").ok_or(Refusal::Payload)? {
        "init" => Operation::Init,
        "approve" => Operation::Approve,
        "recover" => Operation::Recover,
        "revoke" => Operation::Revoke,
        "sharing" => Operation::Sharing,
        _ => return Err(Refusal::Kind),
    };
    Ok(State {
        account: text("account")?,
        seq: fields.integer("seq").ok_or(Refusal::Payload)?,
        prev: text("prev")?,
        kind,
        signer: text("signer")?,
        root: text("root")?,
        recovery: text("recovery")?,
        recovery_box: text("recovery_box")?,
        recovery_wrap: text("recovery_wrap")?,
        epoch: fields.integer("epoch").ok_or(Refusal::Payload)?,
        devices: devices
            .iter()
            .map(read_device)
            .collect::<Option<_>>()
            .ok_or(Refusal::Payload)?,
        inbox: fields
            .optional_string("inbox")
            .ok_or(Refusal::Payload)?
            .map(str::to_owned),
    })
}

fn read_device(value: &Value) -> Option<Device> {
    let fields = Fields::of(value, DEVICE_FIELDS)?;
    Some(Device {
        id: fields.string("id")?.to_owned(),
        name: fields.string("name")?.to_owned(),
        sign_key: fields.string("sign")?.to_owned(),
        box_key: fields.string("box")?.to_owned(),
        wrap: fields.string("wrap")?.to_owned(),
    })
}

/// Every key in a state is a key and every wrapper has a wrapper's shape,
/// whatever the operation. None of them is opened here.
fn check_keys(state: &State) -> Result<(), Refusal> {
    let key = |text: &str| decode_exact::<32>(text).map(drop).map_err(|_| Refusal::Key);
    key(&state.root)?;
    key(&state.recovery)?;
    if let Some(inbox) = &state.inbox {
        key(inbox)?;
    }
    // The root key's seed and the recovery key under the recovery secret:
    // a nonce, 64 bytes and a tag.
    decode_exact::<92>(&state.recovery_box).map_err(|_| Refusal::Wrapper)?;
    wrapper(&state.recovery_wrap)?;
    let mut seen = HashSet::new();
    for device in &state.devices {
        key(&device.sign_key)?;
        key(&device.box_key)?;
        wrapper(&device.wrap)?;
        let id = device_id(&device.sign_key, &device.box_key);
        if device.id != id || !seen.insert(&device.id) || device.name.len() > MAX_DEVICE_NAME {
            return Err(Refusal::Device);
        }
    }
    Ok(())
}

/// A wrapped vault key: the sender's one-time X25519 key, a dot, then a
/// nonce, the 32-byte key and a tag.
fn wrapper(text: &str) -> Result<(), Refusal> {
    wrap::split(text).map(drop).ok_or(Refusal::Wrapper)
}
