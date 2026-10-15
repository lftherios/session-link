//! An account's devices and its vault, as this machine takes part in them:
//! setting up recovery, asking to be approved and approving, recovering with
//! the recovery key, revoking a device, and backing up the links this
//! machine published. The service stores signed statements and ciphertext;
//! every private key, the vault key and what the vault holds stay here.
//!
//! Each action reads the record the service holds, verifies it whole and
//! checks it against this device's pins before using it. A change is one
//! event and the vault written for it, sent with the head and revision it
//! was made from; the service takes it only if nothing moved in between.
//! Go's client (`go/internal/identity/client.go`) is the contract, and the
//! actions and messages are its.

use std::fmt;
use std::fs::{self, File, OpenOptions, TryLockError};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use reqwest::Response;
use ring::rand::{SecureRandom, SystemRandom};
use serde_json::{Map, Value, json};
use slink_local::create_private_dir;
use slink_protocol::encoding::encode;
use slink_protocol::{
    AgreementKey, Kind, Operation, Record, Recovery, Request, Signed, SigningKey, State, Vault,
    Verified, code_matches, confirmation, misspelt, open_recovery, recovery_token, verify_record,
    wrap,
};
use tokio::sync::{Mutex, MutexGuard};

use crate::device::{self, Device};
use crate::origin::origin;
use crate::transport::client;
use crate::vault::{Contents, MAX_PLAINTEXT, Receipt};

/// The most an answer may hold: the record and its pending requests, with
/// room for the list of shares the account owns.
pub(crate) const LIMIT: usize = 12 * 1024 * 1024;
/// The longest device name, in bytes.
const MAX_NAME: usize = 80;

/// One action at a time in this process; the lock file keeps other
/// processes out, and the service's compare-and-swap other machines.
static OPERATIONS: Mutex<()> = Mutex::const_new(());

/// Why an action did not happen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The service refused the request with this status.
    Service {
        status: u16,
        message: String,
    },
    /// This device is not approved, and the action needs it to be.
    Locked,
    /// A setup made here never reached the service, and the account now has
    /// an identity made elsewhere.
    SetupConflict,
    Refused(String),
}

pub(crate) fn refused(message: &str) -> Error {
    Error::Refused(message.to_owned())
}

/// The object `value` is, unless one of its keys is one of `names` in
/// another case: Go would read that key as the field, and this does not.
pub(crate) fn exact<'a>(value: &'a Value, names: &[&str]) -> Option<&'a Map<String, Value>> {
    let fields = value.as_object()?;
    (!fields.keys().any(|key| misspelt(key, names))).then_some(fields)
}

impl Error {
    /// The status a page is told: the service's when it is about signing in,
    /// a conflict or a limit, and otherwise 422.
    pub fn status(&self) -> u16 {
        match self {
            Error::Service { status, .. } if matches!(status, 401 | 403 | 409 | 429) => *status,
            _ => 422,
        }
    }

    /// What a page can act on: `setup_conflict` offers to discard the setup.
    pub fn code(&self) -> Option<&'static str> {
        matches!(self, Error::SetupConflict).then_some("setup_conflict")
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Error::Service { message, .. } | Error::Refused(message) => f.write_str(message),
            Error::Locked => f.write_str("this device needs approval or your recovery key"),
            Error::SetupConflict => f.write_str(
                "this account already has a recovery identity that this device did not create",
            ),
        }
    }
}

impl std::error::Error for Error {}

impl From<slink_protocol::Error> for Error {
    fn from(error: slink_protocol::Error) -> Error {
        Error::Refused(error.to_string())
    }
}

/// What a page sends with an action.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Input {
    /// What to call this device.
    pub name: String,
    /// The device to approve or revoke.
    pub id: String,
    /// The code the device to approve shows.
    pub code: String,
    pub recovery_key: String,
}

/// What a page is shown.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub server: String,
    pub account: String,
    /// `setup` before the account has an identity, `locked` until this
    /// device is approved, then `ready`.
    pub state: &'static str,
    pub device_id: String,
    /// Each approved device: its id, its name, and whether it is this one.
    pub devices: Vec<(String, String, bool)>,
    /// Each pending request: the device's id and its name. Go adds the
    /// request's code, which is what the person is asked to type from the
    /// other device, so the page is not given it.
    pub requests: Vec<(String, String)>,
    pub shares: Vec<Receipt>,
    /// The code of this device's own pending request.
    pub confirmation: String,
    /// The recovery key of a setup made here, when it is asked for.
    pub recovery_key: String,
    pub recovery_pending: bool,
}

impl Status {
    fn new(server: &str, account: &str) -> Status {
        Status {
            server: server.to_owned(),
            account: account.to_owned(),
            state: "setup",
            device_id: String::new(),
            devices: Vec::new(),
            requests: Vec::new(),
            shares: Vec::new(),
            confirmation: String::new(),
            recovery_key: String::new(),
            recovery_pending: false,
        }
    }

    /// The fields Go's client answers with.
    pub fn to_json(&self) -> Value {
        let mut fields = Map::new();
        fields.insert("server".into(), json!(self.server));
        fields.insert("account".into(), json!(self.account));
        fields.insert("state".into(), json!(self.state));
        if !self.device_id.is_empty() {
            fields.insert("device_id".into(), json!(self.device_id));
        }
        let devices = self
            .devices
            .iter()
            .map(|(id, name, current)| json!({"id": id, "name": name, "current": current}));
        fields.insert("devices".into(), Value::Array(devices.collect()));
        let requests = self
            .requests
            .iter()
            .map(|(id, name)| json!({"id": id, "name": name}));
        fields.insert("requests".into(), Value::Array(requests.collect()));
        let shares = self.shares.iter().map(|receipt| {
            json!({
                "server": receipt.server,
                "account": receipt.account,
                "key": receipt.key,
                "sha256": receipt.sha256,
                "url": receipt.url,
            })
        });
        fields.insert("shares".into(), Value::Array(shares.collect()));
        for (name, value) in [
            ("confirmation", &self.confirmation),
            ("recovery_key", &self.recovery_key),
        ] {
            if !value.is_empty() {
                fields.insert(name.into(), json!(value));
            }
        }
        fields.insert("recovery_pending".into(), json!(self.recovery_pending));
        Value::Object(fields)
    }
}

/// A write to the record: an event, if there is one, and the vault made for
/// it, against the head and revision they were made from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Update {
    pub expected_head: String,
    pub expected_revision: u64,
    pub event: Option<Signed>,
    pub vault: Signed,
}

impl Update {
    pub fn to_json(&self) -> Value {
        let mut fields = Map::new();
        fields.insert("expected_head".into(), json!(self.expected_head));
        fields.insert("expected_revision".into(), json!(self.expected_revision));
        if let Some(event) = &self.event {
            fields.insert("event".into(), signed_json(event));
        }
        fields.insert("vault".into(), signed_json(&self.vault));
        Value::Object(fields)
    }

    pub fn from_json(value: &Value) -> Option<Update> {
        Some(Update {
            expected_head: value["expected_head"].as_str()?.to_owned(),
            expected_revision: value["expected_revision"].as_u64()?,
            event: match &value["event"] {
                Value::Null => None,
                event => Some(signed(event)?),
            },
            vault: signed(&value["vault"])?,
        })
    }
}

/// A statement from an object of two texts; a missing one is empty, as Go
/// reads it, and fails verification.
pub(crate) fn signed(value: &Value) -> Option<Signed> {
    let text = |name: &str| match &value[name] {
        Value::Null => Some(String::new()),
        Value::String(text) => Some(text.clone()),
        _ => None,
    };
    value.is_object().then_some(())?;
    Some(Signed {
        payload: text("payload")?,
        signature: text("signature")?,
    })
}

pub(crate) fn signed_json(signed: &Signed) -> Value {
    json!({"payload": signed.payload, "signature": signed.signature})
}

/// What the service answers: the account the key is for, the record it
/// holds for it, if any, and the approval requests waiting.
pub(crate) struct Remote {
    pub(crate) account: String,
    pub(crate) record: Option<Record>,
    requests: Vec<Signed>,
}

impl Remote {
    fn read(value: &Value) -> Option<Remote> {
        let list = |value: &Value| match value {
            Value::Null => Some(Vec::new()),
            Value::Array(items) => items.iter().map(signed).collect(),
            _ => None,
        };
        let record = match &value["record"] {
            Value::Null => None,
            record => Some(Record {
                events: list(&record["events"])?,
                vault: signed(&record["vault"])?,
            }),
        };
        Some(Remote {
            account: value["account"].as_str()?.to_owned(),
            record,
            requests: list(&value["requests"])?,
        })
    }
}

pub(crate) fn random<const N: usize>() -> Result<[u8; N], Error> {
    let mut bytes = [0; N];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| refused("no randomness for new keys"))?;
    Ok(bytes)
}

pub(crate) fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |since| since.as_millis() as u64)
}

/// `key` wrapped to `recipient` for the state `context` names, under a
/// one-time key made for it.
fn wrap_to(context: &str, recipient: &str, key: &[u8; 32]) -> Result<String, Error> {
    let ephemeral = AgreementKey::from_bytes(random()?);
    Ok(wrap(recipient, key, context, &ephemeral, random()?)?)
}

/// The name a person gave, trimmed, at most 80 bytes of whole characters.
fn device_name(name: &str) -> String {
    let name = name.trim();
    if name.is_empty() {
        return "My device".to_owned();
    }
    let mut end = name.len().min(MAX_NAME);
    while !name.is_char_boundary(end) {
        end -= 1;
    }
    name[..end].to_owned()
}

/// `data` sealed for `next` and signed by `signer`, as a write against
/// `old`, the record it was made from.
fn prepare(
    server: &str,
    old: Option<&Verified>,
    next: &State,
    event: Option<Signed>,
    data: &Contents,
    key: &[u8; 32],
    signer: &SigningKey,
) -> Result<Update, Error> {
    if !data.named_fit(server, next) {
        return Err(refused("invalid private sharing backup"));
    }
    let raw = data.encode();
    if raw.len() > MAX_PLAINTEXT {
        return Err(refused("key vault exceeds the backup limit"));
    }
    let (revision, head) = old.map_or((0, String::new()), |old| {
        (old.vault.revision, old.head.clone())
    });
    let vault = Vault {
        account: next.account.clone(),
        revision: revision + 1,
        head: match &event {
            Some(event) => event.hash(Kind::Event)?,
            None => head.clone(),
        },
        epoch: next.epoch,
        signer: next.signer.clone(),
        data: next.seal_vault(key, &raw, random()?),
    };
    Ok(Update {
        expected_head: head,
        expected_revision: revision,
        event,
        vault: Signed::sign(Kind::Vault, &vault.encode(), signer)?,
    })
}

/// This machine's part in one account on one server.
pub struct Identity {
    pub(crate) home: PathBuf,
    server: Option<String>,
    pub(crate) api_key: String,
}

/// Held while this process changes the keys of a home: the lock in the
/// process and the one on `identity/.lock`. Fields drop in order, so the
/// file is let go first and the next task in this process finds it free.
pub(crate) struct Exclusive {
    _file: File,
    _process: MutexGuard<'static, ()>,
}

impl Identity {
    /// For the account `api_key` names on `server`, with this machine's keys
    /// in `home`.
    pub fn new(home: &Path, server: &str, api_key: &str) -> Identity {
        Identity {
            home: home.to_owned(),
            server: origin(server).ok(),
            api_key: api_key.to_owned(),
        }
    }

    pub(crate) fn server(&self) -> Result<&str, Error> {
        self.server
            .as_deref()
            .ok_or_else(|| refused("invalid encrypted identity server"))
    }

    /// Refuses before anything is sent when there is no key to send.
    pub(crate) fn signed_in(&self) -> Result<(), Error> {
        if self.api_key.is_empty() {
            return Err(Error::Service {
                status: 401,
                message: "run slink login first".to_owned(),
            });
        }
        Ok(())
    }

    /// Waits for this process's other actions, then takes the lock file
    /// without waiting; `busy` is what another process holding it is told.
    pub(crate) async fn exclusive(&self, busy: &str) -> Result<Exclusive, Error> {
        let process = OPERATIONS.lock().await;
        let dir = self.home.join("identity");
        create_private_dir(&dir).map_err(|error| refused(&error.to_string()))?;
        let mut options = OpenOptions::new();
        options.create(true).truncate(false).read(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let file = options
            .open(dir.join(".lock"))
            .map_err(|error| refused(&error.to_string()))?;
        match file.try_lock() {
            Ok(()) => Ok(Exclusive {
                _process: process,
                _file: file,
            }),
            Err(TryLockError::WouldBlock) => Err(refused(busy)),
            Err(TryLockError::Error(error)) => Err(refused(&error.to_string())),
        }
    }

    /// Takes `action` with what a page sent: `status`, `setup`,
    /// `confirm-recovery`, `discard-setup`, `request`, `recover`, `approve`,
    /// `revoke`, `sharing` or `sync`.
    pub async fn handle(&self, action: &str, input: &Input) -> Result<Status, Error> {
        let server = self.server()?;
        self.signed_in()?;
        let _held = self
            .exclusive("another slink process is updating device keys; retry shortly")
            .await?;
        // A backup that lost a race is made again from the newer vault.
        for _ in 0..3 {
            match self.once(server, action, input).await {
                Err(Error::Service { status: 409, .. }) if action == "sync" => continue,
                other => return other,
            }
        }
        Err(refused("vault is changing on another device; retry backup"))
    }

    /// Whether the key of a link just published went into the account's
    /// vault: `synced`, `locked` on a device not approved yet, `setup` when
    /// this machine has no device for the account, or `failed`. The link
    /// and its receipt are kept whatever this says.
    pub async fn backup(&self, account: &str) -> &'static str {
        let Ok(server) = self.server() else {
            return "failed";
        };
        if account.is_empty() {
            return "setup";
        }
        match device::load(&device::file(&self.home, server, account)) {
            Err(_) => "failed",
            Ok(None) => "setup",
            Ok(Some(device)) if device.root.is_empty() => "setup",
            Ok(Some(_)) => match self.handle("sync", &Input::default()).await {
                Ok(_) => "synced",
                Err(Error::Locked) => "locked",
                Err(_) => "failed",
            },
        }
    }

    /// The record the service holds, or, with `body`, the record after it
    /// took the write or the request in `body`.
    pub(crate) async fn request(&self, server: &str, body: Option<Value>) -> Result<Remote, Error> {
        let unreachable = || refused("cannot reach encrypted key backup");
        let http = client(Duration::from_secs(45)).map_err(|_| unreachable())?;
        let url = format!("{server}/api/identity");
        let builder = match body {
            Some(body) => http.post(url).body(slink_format::encode(&body)),
            None => http.get(url),
        };
        let response = builder
            .header("authorization", format!("Bearer {}", self.api_key))
            .header("content-type", "application/json")
            .send()
            .await
            .map_err(|_| unreachable())?;
        let status = response.status().as_u16();
        let invalid = || refused("invalid identity response");
        let raw = read(response, LIMIT).await.ok_or_else(invalid)?;
        let value = serde_json::from_slice::<Value>(&raw).ok();
        if status != 200 {
            let message = value
                .as_ref()
                .and_then(|value| value["error"]["message"].as_str())
                .unwrap_or_default();
            return Err(Error::Service {
                status,
                message: format!("{message} (HTTP {status})"),
            });
        }
        value
            .as_ref()
            .and_then(Remote::read)
            .filter(|remote| !remote.account.is_empty() && remote.account.len() <= 128)
            .ok_or_else(invalid)
    }

    /// A new identity for `account` with `device` as its first device: the
    /// recovery keys, the first event and vault, pinned and kept on the
    /// device before anything is sent.
    fn genesis(
        &self,
        server: &str,
        account: &str,
        device: &mut Device,
        input: &Input,
    ) -> Result<(), Error> {
        let recovery = Recovery {
            root: SigningKey::from_seed(random()?),
            agreement: AgreementKey::from_bytes(random()?),
        };
        let (secret, key) = (random::<32>()?, random::<32>()?);
        let root = recovery.root.public();
        let mut state = State {
            account: account.to_owned(),
            seq: 0,
            prev: String::new(),
            kind: Operation::Init,
            signer: "root".to_owned(),
            root: root.clone(),
            recovery: recovery.agreement.public(),
            recovery_box: recovery.seal(account, &secret, random()?),
            recovery_wrap: String::new(),
            epoch: 1,
            devices: Vec::new(),
            inbox: None,
        };
        let context = state.context();
        let mut first = device.public(&device_name(&input.name));
        first.wrap = wrap_to(&context, &first.box_key, &key)?;
        state.devices.push(first);
        state.recovery_wrap = wrap_to(&context, &state.recovery, &key)?;
        let event = Signed::sign(Kind::Event, &state.encode(), &recovery.root)?;
        let head = event.hash(Kind::Event)?;
        let data = Contents::default().merge(&self.home, server, account, &root)?;
        let update = prepare(
            server,
            None,
            &state,
            Some(event),
            &data,
            &key,
            &recovery.root,
        )?;
        device.root = root.clone();
        device.seq = 0;
        device.head = head;
        device.revision = 1;
        device.pending_setup = Some(update);
        device.pending_recovery = recovery_token(account, &root, &secret);
        Ok(())
    }

    /// One attempt at `action`, under the locks [`Identity::handle`] takes.
    pub(crate) async fn once(
        &self,
        server: &str,
        action: &str,
        input: &Input,
    ) -> Result<Status, Error> {
        let mut remote = self.request(server, None).await?;
        let file = device::file(&self.home, server, &remote.account);
        let mut device = device::load(&file)?;
        let mut result = Status::new(server, &remote.account);
        if remote.record.is_none() {
            if device
                .as_ref()
                .is_some_and(|device| !device.root.is_empty() && device.pending_setup.is_none())
            {
                return Err(refused(
                    "encrypted identity is missing from the server; refusing to replace it",
                ));
            }
            match action {
                "status" => return Ok(result),
                "setup" => {}
                _ => return Err(refused("set up recovery on your first device")),
            }
            let mut new = match device {
                Some(device) => device,
                None => Device::new(random()?, random()?),
            };
            if new.pending_setup.is_none() {
                // The identity is pinned and the exact request kept before
                // the upload, so a retry cannot change the root.
                self.genesis(server, &remote.account, &mut new, input)?;
                device::save(&file, &new)?;
            }
            let account = remote.account.clone();
            let body = new.pending_setup.as_ref().map(Update::to_json);
            remote = self.request(server, body).await?;
            if remote.account != account || remote.record.is_none() {
                return Err(refused(
                    "server did not acknowledge the initial encrypted identity",
                ));
            }
            device = Some(new);
        }
        let Some(record) = remote.record.take() else {
            unreachable!("a record was read or acknowledged");
        };
        let verified = verify_record(&record, &remote.account)?;
        let mut action = action;
        if action == "discard-setup"
            && device
                .as_ref()
                .is_some_and(|device| device.setup_conflict(&verified.state))
        {
            // The keys go with the setup: the service has seen them. This
            // device then joins as any new device does.
            fs::remove_file(&file).map_err(|error| refused(&error.to_string()))?;
            device = None;
            action = "status";
        }
        if let Some(device) = device.as_mut() {
            device.pin(&record, &verified)?;
            device::save(&file, device)?;
        }
        if action == "discard-setup" {
            return Err(refused(
                "this device has no unfinished recovery setup to discard",
            ));
        }
        if device.is_none() && matches!(action, "request" | "recover") {
            let mut new = Device::new(random()?, random()?);
            new.pin(&record, &verified)?;
            device::save(&file, &new)?;
            device = Some(new);
        }

        let state = &verified.state;
        let own = device
            .as_ref()
            .and_then(|device| state.device(&device.id()))
            .cloned();
        if let Some(device) = &device {
            result.device_id = device.id();
            result.recovery_pending = !device.pending_recovery.is_empty();
        }
        result.state = "locked";
        result.devices = state
            .devices
            .iter()
            .map(|listed| {
                let current = listed.id == result.device_id;
                (listed.id.clone(), listed.name.clone(), current)
            })
            .collect();
        let now = now();
        for signed in &remote.requests {
            let Ok(request) = Request::verify(signed) else {
                continue;
            };
            if request.account != state.account
                || request.root != state.root
                || !request.pending(now)
            {
                continue;
            }
            if request.id() == result.device_id {
                result.confirmation = confirmation(signed)?;
            }
            result.requests.push((request.id(), request.name));
        }

        let Some(own) = own else {
            return match (action, device) {
                ("request", Some(mut device)) => {
                    let request = Request {
                        account: state.account.clone(),
                        root: state.root.clone(),
                        name: device_name(&input.name),
                        sign_key: device.sign.public(),
                        box_key: device.agreement.public(),
                        nonce: encode(&random::<24>()?),
                        created: now,
                    };
                    let signed = Signed::sign(Kind::Request, &request.encode(), &device.sign)?;
                    device.request = Some(signed.clone());
                    device::save(&file, &device)?;
                    self.request(server, Some(json!({"request": signed_json(&signed)})))
                        .await?;
                    result.confirmation = confirmation(&signed)?;
                    Ok(result)
                }
                ("recover", _) => self.recover(server, &record, &verified, &file, input).await,
                ("status", _) => Ok(result),
                _ => Err(Error::Locked),
            };
        };
        let Some(mut device) = device else {
            unreachable!("an approved device is this one");
        };
        result.state = "ready";
        let mut key = state.unwrap_key(&device.agreement, &own.wrap)?;
        let mut data = Contents::open(&key, &verified, server)?;
        data.import(&self.home)?;
        result.shares = data.shares.clone();
        let kind = match action {
            "setup" if !device.pending_recovery.is_empty() => {
                result.recovery_key = device.pending_recovery.clone();
                return Ok(result);
            }
            "confirm-recovery" => {
                device.pending_recovery.clear();
                device::save(&file, &device)?;
                result.recovery_pending = false;
                return Ok(result);
            }
            "status" => return Ok(result),
            "sharing" if state.inbox.is_some() => return Ok(result),
            "sync" => None,
            "approve" => Some(Operation::Approve),
            "revoke" => Some(Operation::Revoke),
            "sharing" => Some(Operation::Sharing),
            _ => return Err(refused("unknown device action")),
        };
        data = data.merge(&self.home, server, &state.account, &state.root)?;
        let mut next = state.clone();
        next.signer = own.id.clone();
        match kind {
            Some(Operation::Sharing) => data.rotate_inbox(&mut next, random()?),
            Some(Operation::Approve) => {
                // As Go does, the first request with that id is the one, and
                // it must verify: a second copy is not looked for.
                let found = remote.requests.iter().find(|signed| {
                    Request::read(signed).is_ok_and(|request| request.id() == input.id)
                });
                let found = found.and_then(|signed| Some((signed, Request::verify(signed).ok()?)));
                let Some((_, request)) = found.filter(|(signed, request)| {
                    request.account == next.account
                        && request.root == next.root
                        && code_matches(&input.code, signed)
                }) else {
                    return Err(refused(
                        "device code does not match; check the code on your new device",
                    ));
                };
                let wrapper = wrap_to(&next.context(), &request.box_key, &key)?;
                next.devices.push(slink_protocol::Device {
                    id: request.id(),
                    name: request.name,
                    sign_key: request.sign_key,
                    box_key: request.box_key,
                    wrap: wrapper,
                });
            }
            Some(Operation::Revoke) => {
                if input.id == own.id {
                    return Err(refused("revoke this device from another approved device"));
                }
                if next.device(&input.id).is_none() {
                    return Err(refused("device is no longer approved"));
                }
                // A new epoch and a new vault key, wrapped to every device
                // that stays and to the recovery key; a new incoming-share
                // key too, so the removed device opens nothing sent later.
                next.epoch += 1;
                if next.inbox.is_some() {
                    data.rotate_inbox(&mut next, random()?);
                }
                key = random()?;
                let context = next.context();
                next.devices.retain(|listed| listed.id != input.id);
                for listed in &mut next.devices {
                    listed.wrap = wrap_to(&context, &listed.box_key, &key)?;
                }
                next.recovery_wrap = wrap_to(&context, &next.recovery, &key)?;
            }
            _ => {}
        }
        let event = match kind {
            Some(operation) => {
                next.kind = operation;
                next.seq += 1;
                next.prev = verified.head.clone();
                Some(Signed::sign(Kind::Event, &next.encode(), &device.sign)?)
            }
            None => None,
        };
        let update = prepare(
            server,
            Some(&verified),
            &next,
            event,
            &data,
            &key,
            &device.sign,
        )?;
        self.request(server, Some(update.to_json())).await?;
        device.wrote(&update, next.seq)?;
        device::save(&file, &device)?;
        Box::pin(self.once(server, "status", &Input::default())).await
    }

    /// Enrolls fresh keys on this machine with the recovery key: the
    /// recovery package opened here, the vault key unwrapped from the
    /// recovery wrapper, and an event the root key signs. Fresh keys also
    /// cover a machine whose device was revoked.
    async fn recover(
        &self,
        server: &str,
        record: &Record,
        verified: &Verified,
        file: &Path,
        input: &Input,
    ) -> Result<Status, Error> {
        let state = &verified.state;
        let recovery = open_recovery(&input.recovery_key, state)?;
        let key = state.unwrap_key(&recovery.agreement, &state.recovery_wrap)?;
        let data = Contents::open(&key, verified, server)?.merge(
            &self.home,
            server,
            &state.account,
            &state.root,
        )?;
        let mut device = Device::new(random()?, random()?);
        device.pin(record, verified)?;
        device::save(file, &device)?;
        let mut next = state.clone();
        let mut added = device.public(&device_name(&input.name));
        added.wrap = wrap_to(&next.context(), &added.box_key, &key)?;
        next.devices.push(added);
        next.kind = Operation::Recover;
        next.signer = "root".to_owned();
        next.seq += 1;
        next.prev = verified.head.clone();
        let event = Signed::sign(Kind::Event, &next.encode(), &recovery.root)?;
        let update = prepare(
            server,
            Some(verified),
            &next,
            Some(event),
            &data,
            &key,
            &recovery.root,
        )?;
        self.request(server, Some(update.to_json())).await?;
        device.wrote(&update, next.seq)?;
        device::save(file, &device)?;
        Box::pin(self.once(server, "status", &Input::default())).await
    }
}

/// The whole answer, unless it is longer than `limit`.
pub(crate) async fn read(mut response: Response, limit: usize) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.ok()? {
        body.extend_from_slice(&chunk);
        if body.len() > limit {
            return None;
        }
    }
    Some(body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_device_is_named_as_go_names_it() {
        assert_eq!(device_name("  "), "My device");
        assert_eq!(device_name(" Laptop \n"), "Laptop");
        let long = "é".repeat(41);
        assert_eq!(device_name(&long), "é".repeat(40));
        assert_eq!(device_name(&"a".repeat(81)), "a".repeat(80));
    }

    #[test]
    fn a_page_is_told_the_services_status_only_for_sign_in_conflicts_and_limits() {
        for (status, told) in [
            (401, 401),
            (403, 403),
            (409, 409),
            (429, 429),
            (500, 422),
            (404, 422),
        ] {
            let error = Error::Service {
                status,
                message: String::new(),
            };
            assert_eq!(error.status(), told);
        }
        assert_eq!(Error::Locked.status(), 422);
        assert_eq!(Error::SetupConflict.code(), Some("setup_conflict"));
        assert_eq!(Error::Locked.code(), None);
    }

    #[test]
    fn an_update_is_read_back_as_written() {
        let vault = Signed {
            payload: "v".into(),
            signature: "s".into(),
        };
        let mut update = Update {
            expected_head: "h".into(),
            expected_revision: 3,
            event: None,
            vault,
        };
        assert!(update.to_json().get("event").is_none());
        assert_eq!(Update::from_json(&update.to_json()), Some(update.clone()));
        update.event = Some(update.vault.clone());
        assert_eq!(Update::from_json(&update.to_json()), Some(update));
    }
}
