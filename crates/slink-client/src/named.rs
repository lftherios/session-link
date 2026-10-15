//! Sharing with named people: a session sealed here, uploaded with one
//! signed invitation for each email address, and its key granted to each
//! person once they accept from a device of their own.
//!
//! An invitation carries a commitment to a secret; the secret goes only in
//! the link the sender passes on. A person who accepts signs a claim with a
//! device their account approved and proves it with the secret, and this
//! machine checks both against their history before it wraps the key to
//! their incoming-share key. The service carries every statement and could
//! offer an account of its own; it never sees a secret to prove it with.
//!
//! What a share needs until every person has their key is kept in the
//! outbox, `named-shares/<ciphertext hash>.json`, and in the account's vault,
//! so the local viewer can grant while it runs, on this machine or another
//! of the account's. Go's client (`go/internal/identity/named.go`) is the
//! contract, and the messages are its.

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Map, Value, json};
use sha2::{Digest, Sha256};
use slink_local::write_private;
use slink_protocol::encoding::{decode_exact, digest, encode};
use slink_protocol::envelope::{self, MAX_PLAINTEXT};
use slink_protocol::{
    AgreementKey, Invitation, Invite, Kind, Offer, Signed, Verified, commitment, verify_claim,
    verify_record,
};

use crate::device::{self, Device};
use crate::identity::{
    Error, Identity, Input, LIMIT, exact, random, read, refused, signed, signed_json,
};
use crate::publish::{hex, seal, share_id};
use crate::transport::client;
use crate::vault::Contents;

/// What another process holding the lock file is told.
const BUSY: &str = "another slink process is updating private keys; retry shortly";
/// The most people one share is for.
const MAX_RECIPIENTS: usize = 10;
/// How many unfinished shares one background pass looks at.
const BATCH: usize = 8;
/// The fields of an outbox, as Go names them.
const OUTBOX_FIELDS: &[&str] = &[
    "status",
    "server",
    "account",
    "root",
    "id",
    "sha256",
    "key",
    "envelope",
    "invitations",
    "secrets",
];

/// A share for named people as this machine keeps it until each person has
/// their key: in Go's shape, so the vault holds what Go's would.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Outbox {
    /// What the service last said of each invitation, by its id.
    pub status: BTreeMap<String, String>,
    pub server: String,
    pub account: String,
    pub root: String,
    /// The share's id, once the service has acknowledged it.
    pub id: String,
    pub sha256: String,
    pub key: String,
    /// The ciphertext, kept only until the service acknowledges it.
    pub envelope: Vec<u8>,
    pub invitations: Vec<Invitation>,
    /// Each invitation's secret, by its id.
    pub secrets: BTreeMap<String, String>,
}

/// A field Go reads as text: missing or null is empty.
fn text(fields: &Map<String, Value>, name: &str) -> Option<String> {
    match fields.get(name) {
        None | Some(Value::Null) => Some(String::new()),
        Some(Value::String(text)) => Some(text.clone()),
        Some(_) => None,
    }
}

/// A field Go reads as text by text: missing or null is empty.
fn texts(fields: &Map<String, Value>, name: &str) -> Option<BTreeMap<String, String>> {
    match fields.get(name) {
        None | Some(Value::Null) => Some(BTreeMap::new()),
        Some(Value::Object(map)) => map
            .iter()
            .map(|(key, value)| Some((key.clone(), value.as_str()?.to_owned())))
            .collect(),
        Some(_) => None,
    }
}

fn object(map: &BTreeMap<String, String>) -> String {
    let fields = map
        .iter()
        .map(|(key, value)| (key.clone(), Value::String(value.clone())))
        .collect();
    slink_format::encode(&Value::Object(fields))
}

impl Outbox {
    /// From an object with the fields and types Go reads; any other field
    /// is dropped, as Go drops it.
    pub fn read(value: &Value) -> Option<Outbox> {
        let fields = exact(value, OUTBOX_FIELDS)?;
        let invitations = match fields.get("invitations") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(items)) => items
                .iter()
                .map(|item| {
                    let fields = exact(item, &["id", "email", "commitment"])?;
                    Some(Invitation {
                        id: text(fields, "id")?,
                        email: text(fields, "email")?,
                        commitment: text(fields, "commitment")?,
                    })
                })
                .collect::<Option<_>>()?,
            Some(_) => return None,
        };
        Some(Outbox {
            status: texts(fields, "status")?,
            server: text(fields, "server")?,
            account: text(fields, "account")?,
            root: text(fields, "root")?,
            id: text(fields, "id")?,
            sha256: text(fields, "sha256")?,
            key: text(fields, "key")?,
            envelope: STANDARD.decode(text(fields, "envelope")?).ok()?,
            invitations,
            secrets: texts(fields, "secrets")?,
        })
    }

    /// As Go's `json.Marshal` writes it, the empty status and envelope left
    /// out.
    pub fn encode(&self) -> String {
        let quoted = |value: &str| slink_format::encode(&Value::String(value.to_owned()));
        let mut fields = Vec::new();
        if !self.status.is_empty() {
            fields.push(format!("\"status\":{}", object(&self.status)));
        }
        for (name, value) in [
            ("server", &self.server),
            ("account", &self.account),
            ("root", &self.root),
            ("id", &self.id),
            ("sha256", &self.sha256),
            ("key", &self.key),
        ] {
            fields.push(format!("\"{name}\":{}", quoted(value)));
        }
        if !self.envelope.is_empty() {
            let envelope = STANDARD.encode(&self.envelope);
            fields.push(format!("\"envelope\":{}", quoted(&envelope)));
        }
        let invitations: Vec<String> = self
            .invitations
            .iter()
            .map(|invitation| {
                format!(
                    "{{\"id\":{},\"email\":{},\"commitment\":{}}}",
                    quoted(&invitation.id),
                    quoted(&invitation.email),
                    quoted(&invitation.commitment)
                )
            })
            .collect();
        fields.push(format!("\"invitations\":[{}]", invitations.join(",")));
        fields.push(format!("\"secrets\":{}", object(&self.secrets)));
        format!("{{{}}}", fields.join(","))
    }

    /// Whether this is a share Go would keep for this identity: acknowledged
    /// on this server, for this account and root key, with one to ten
    /// invitations to addresses, each with the secret its commitment was
    /// made from, and no ciphertext.
    pub fn valid(&self, server: &str, account: &str, root: &str) -> bool {
        if self.server != server
            || self.account != account
            || self.root != root
            || !share_id(&self.id)
            || self.sha256.len() != 64
            || !self.sha256.bytes().all(|byte| byte.is_ascii_hexdigit())
            || !self.envelope.is_empty()
            || !(1..=MAX_RECIPIENTS).contains(&self.invitations.len())
            || decode_exact::<32>(&self.key).is_err()
        {
            return false;
        }
        let mut seen = Vec::new();
        self.invitations.iter().all(|invitation| {
            let fresh = !seen.contains(&&invitation.id);
            seen.push(&invitation.id);
            let secret = self.secrets.get(&invitation.id).map_or("", String::as_str);
            fresh
                && invitation.email.len() <= 254
                && email_address(&invitation.email)
                && decode_exact::<24>(&invitation.id).is_ok()
                && decode_exact::<32>(secret)
                    .is_ok_and(|secret| commitment(&secret) == invitation.commitment)
        })
    }

    /// The share as a page is shown it. A person still to accept gets the
    /// invitation link, with its secret; once they have accepted, the secret
    /// has done its work and their link goes without it.
    pub fn result(&self) -> Shared {
        let url = format!("{}/n/{}", self.server, self.id);
        let recipients = self
            .invitations
            .iter()
            .map(|invitation| {
                let status = self
                    .status
                    .get(&invitation.id)
                    .filter(|status| !status.is_empty())
                    .map_or("waiting", String::as_str)
                    .to_owned();
                // The invitation's public id picks the right one when one
                // account holds several.
                let mut link = format!("{url}?invite={}", invitation.id);
                if status == "waiting" {
                    let secret = self.secrets.get(&invitation.id).map_or("", String::as_str);
                    link.push_str(&format!("#invite={secret}&sender={}", self.root));
                }
                Recipient {
                    id: invitation.id.clone(),
                    email: invitation.email.clone(),
                    url: link,
                    status,
                }
            })
            .collect();
        Shared {
            id: self.id.clone(),
            url,
            recipients,
            error: String::new(),
        }
    }
}

/// One person a share is for, as a page is shown them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Recipient {
    pub id: String,
    pub email: String,
    /// The link to send them.
    pub url: String,
    /// `waiting`, `accepted`, `ready`, `expired` or `revoked`.
    pub status: String,
}

impl Recipient {
    pub fn to_json(&self) -> Value {
        json!({"id": self.id, "email": self.email, "url": self.url, "status": self.status})
    }
}

/// A share for named people, as a page is shown it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Shared {
    pub id: String,
    pub url: String,
    pub recipients: Vec<Recipient>,
    /// What went wrong with it, when something did.
    pub error: String,
}

impl Shared {
    /// The fields Go's client answers with.
    pub fn to_json(&self) -> Value {
        let mut fields = Map::new();
        fields.insert("id".into(), json!(self.id));
        fields.insert("url".into(), json!(self.url));
        let recipients = self.recipients.iter().map(Recipient::to_json).collect();
        fields.insert("recipients".into(), Value::Array(recipients));
        if !self.error.is_empty() {
            fields.insert("error".into(), json!(self.error));
        }
        Value::Object(fields)
    }
}

/// Go's pattern for an address a share is for, lower case only:
/// `^[a-z0-9.!#$%&'*+/=?^_`{|}~-]+@[a-z0-9](?:[a-z0-9.-]*[a-z0-9])?\.[a-z]{2,63}$`.
pub(crate) fn email_address(text: &str) -> bool {
    let alnum = |byte: u8| byte.is_ascii_lowercase() || byte.is_ascii_digit();
    let Some((local, domain)) = text.split_once('@') else {
        return false;
    };
    // The top-level name has no dot, so it follows the last one.
    let Some((name, top)) = domain.rsplit_once('.') else {
        return false;
    };
    !local.is_empty()
        && local
            .bytes()
            .all(|byte| alnum(byte) || b".!#$%&'*+/=?^_`{|}~-".contains(&byte))
        && name.bytes().next().is_some_and(alnum)
        && name.bytes().last().is_some_and(alnum)
        && name
            .bytes()
            .all(|byte| alnum(byte) || matches!(byte, b'.' | b'-'))
        && (2..=63).contains(&top.len())
        && top.bytes().all(|byte| byte.is_ascii_lowercase())
}

/// The addresses a person typed, trimmed, in lower case, each once and in
/// order: one to ten of them, each one an address.
fn recipients(values: &[String]) -> Result<Vec<String>, Error> {
    if !(1..=MAX_RECIPIENTS).contains(&values.len()) {
        return Err(refused("choose between one and ten email recipients"));
    }
    let mut out = Vec::new();
    for value in values {
        let email = value.trim().to_lowercase();
        if email.len() > 254 || !email_address(&email) {
            return Err(refused("enter a valid email address for each recipient"));
        }
        out.push(email);
    }
    out.sort();
    out.dedup();
    Ok(out)
}

fn io_error(error: io::Error) -> Error {
    refused(&error.to_string())
}

fn outbox_file(home: &Path, name: &str) -> PathBuf {
    home.join("named-shares").join(format!("{name}.json"))
}

fn save(file: &Path, outbox: &Outbox) -> Result<(), Error> {
    write_private(file, outbox.encode().as_bytes()).map_err(io_error)
}

/// The shares this machine keeps for this identity, by file name. A file for
/// another server or account, or one that cannot be read or checked, stays
/// where it is and is left out, so that no one file can stop a listing, a
/// grant or a backup.
pub(crate) fn read_outbox(
    home: &Path,
    server: &str,
    account: &str,
    root: &str,
) -> Result<Vec<Outbox>, Error> {
    let entries = match fs::read_dir(home.join("named-shares")) {
        Ok(entries) => entries.collect::<io::Result<Vec<_>>>().map_err(io_error)?,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(io_error(error)),
    };
    let mut named: Vec<(String, PathBuf)> = entries
        .iter()
        .filter(|entry| entry.file_type().is_ok_and(|kind| !kind.is_dir()))
        .filter_map(|entry| Some((entry.file_name().into_string().ok()?, entry.path())))
        .filter(|(name, _)| name.ends_with(".json") && !name.starts_with("pending-"))
        .collect();
    named.sort();
    Ok(named
        .iter()
        .filter_map(|(_, path)| {
            let value: Value = serde_json::from_slice(&fs::read(path).ok()?).ok()?;
            Outbox::read(&value)
        })
        .filter(|item| item.valid(server, account, root))
        .collect())
}

/// What the service answers about one of this account's shares.
struct Remote {
    id: String,
    owner: bool,
    sha256: String,
    recipients: Vec<Listed>,
}

/// One person as the service lists them, with their claim once they have
/// accepted, and the history it is checked against when asked for.
struct Listed {
    id: String,
    email: String,
    status: String,
    claim: Option<(Signed, String)>,
    history: Vec<Signed>,
}

impl Remote {
    /// The fields Go reads, with the types it reads them as.
    fn read(value: &Value) -> Option<Remote> {
        let fields = value.as_object()?;
        let list = |value: Option<&Value>| match value {
            None | Some(Value::Null) => Some(Vec::new()),
            Some(Value::Array(items)) => Some(items.clone()),
            Some(_) => None,
        };
        let recipients = list(fields.get("recipients"))?
            .iter()
            .map(|item| {
                let fields = item.as_object()?;
                let claim = match fields.get("claim") {
                    None | Some(Value::Null) => None,
                    Some(claim) => {
                        let proof = text(claim.as_object()?, "proof")?;
                        Some((signed(claim)?, proof))
                    }
                };
                Some(Listed {
                    id: text(fields, "id")?,
                    email: text(fields, "email")?,
                    status: text(fields, "status")?,
                    claim,
                    history: list(fields.get("recipient_history"))?
                        .iter()
                        .map(signed)
                        .collect::<Option<_>>()?,
                })
            })
            .collect::<Option<_>>()?;
        Some(Remote {
            id: text(fields, "id")?,
            owner: match fields.get("owner") {
                None | Some(Value::Null) => false,
                Some(owner) => owner.as_bool()?,
            },
            sha256: text(fields, "sha256")?,
            recipients,
        })
    }
}

/// What a share needs of this machine's part in the account: the record
/// verified and pinned, and this device, approved in it.
struct Keys {
    verified: Verified,
    device: Device,
    /// This device as the history lists it.
    own: slink_protocol::Device,
}

impl Identity {
    /// Shares `text` with the people at `emails`: sealed here, uploaded with
    /// an invitation for each, and kept in the outbox and the vault until
    /// each has their key. Sharing is turned on for the account first if it
    /// is not yet. The answer has each person's link to send them.
    pub async fn publish_named(&self, text: &str, emails: &[String]) -> Result<Shared, Error> {
        let server = self.server()?;
        let _held = self.exclusive(BUSY).await?;
        let recipients = recipients(emails)?;
        let valid = text.len() <= MAX_PLAINTEXT
            && slink_format::parse(text)
                .is_ok_and(|document| slink_format::validate(&document).is_empty());
        if !valid {
            return Err(refused("not a valid supported session document"));
        }
        if !slink_format::scan_for_secrets(text).is_empty() {
            return Err(refused("redact credentials before publishing"));
        }
        self.signed_in()?;
        let keys = self.keys(server, true).await?;
        let state = &keys.verified.state;
        // The same text for the same people is the same ciphertext and the
        // same invitations, so a share whose answer was lost is sent again
        // and the service knows it.
        let pending = outbox_file(
            &self.home,
            &format!(
                "pending-{}",
                digest(
                    format!(
                        "{server}\0{}\0{text}\0{}",
                        state.account,
                        recipients.join("\0")
                    )
                    .as_bytes()
                )
            ),
        );
        let mut saved = match fs::read(&pending) {
            Ok(raw) => {
                let saved = serde_json::from_slice(&raw)
                    .ok()
                    .as_ref()
                    .and_then(Outbox::read)
                    .ok_or_else(|| refused("cannot read pending private share"))?;
                let opened = envelope::decrypt(&saved.envelope, &saved.key);
                if opened.ok().as_deref() != Some(text.as_bytes())
                    || saved.server != server
                    || saved.account != state.account
                    || saved.root != state.root
                {
                    return Err(refused("pending private share failed verification"));
                }
                saved
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                let (sealed, key) = seal(text).map_err(|message| refused(&message))?;
                let mut saved = Outbox {
                    server: server.to_owned(),
                    account: state.account.clone(),
                    root: state.root.clone(),
                    sha256: hex(&Sha256::digest(&sealed)),
                    key,
                    envelope: sealed,
                    ..Outbox::default()
                };
                for email in recipients {
                    let id = encode(&random::<24>()?);
                    let secret = random::<32>()?;
                    saved.invitations.push(Invitation {
                        id: id.clone(),
                        email,
                        commitment: commitment(&secret),
                    });
                    saved.secrets.insert(id, encode(&secret));
                }
                saved
            }
            Err(error) => return Err(io_error(error)),
        };
        save(&pending, &saved)?;
        let key = decode_exact::<32>(&saved.key)?;
        // The share's key for the account's own devices, under its
        // incoming-share key; the people invited get theirs in a grant.
        let wrap = state.wrap_share_key(
            &saved.sha256,
            &key,
            &AgreementKey::from_bytes(random()?),
            random()?,
        )?;
        let mut statements = Vec::new();
        for invitation in &saved.invitations {
            let invite = Invite {
                account: saved.account.clone(),
                root: saved.root.clone(),
                head: keys.verified.head.clone(),
                signer: keys.own.id.clone(),
                sha256: saved.sha256.clone(),
                inbox: state.inbox.clone().unwrap_or_default(),
                epoch: state.epoch,
                wrap: wrap.clone(),
                invitation: invitation.clone(),
            };
            let statement = Signed::sign(Kind::NamedInvite, &invite.encode(), &keys.device.sign)?;
            statements.push(signed_json(&statement));
        }
        // The invitations, then the ciphertext, in one body; the header says
        // where one ends.
        let prefix = slink_format::encode(&Value::Array(statements));
        let mut body = prefix.clone().into_bytes();
        body.extend_from_slice(&saved.envelope);
        let headers = [
            (
                "content-type",
                "application/vnd.session-link.encrypted".to_owned(),
            ),
            ("x-slink-invitations", prefix.len().to_string()),
        ];
        let answer = match self
            .named_request(server, "/api/named-shares", &headers, Some(body))
            .await
        {
            Ok(answer) => answer,
            Err(error) => return Err(discard_deleted(&pending, error)),
        };
        let answer = serde_json::from_slice::<Value>(&answer).ok();
        let field = |name: &str| {
            answer
                .as_ref()
                .and_then(|answer| answer[name].as_str())
                .unwrap_or_default()
                .to_owned()
        };
        let id = field("id");
        if !share_id(&id) || field("sha256") != saved.sha256 || field("account_id") != saved.account
        {
            return Err(refused(
                "server did not acknowledge this private share; retry with the saved local keys",
            ));
        }
        saved.id = id;
        saved.envelope.clear();
        save(&outbox_file(&self.home, &saved.sha256), &saved)?;
        let _ = fs::remove_file(&pending);
        let mut result = saved.result();
        if self.once(server, "sync", &Input::default()).await.is_err() {
            result.error = "Published; private invitation backup needs a retry.".to_owned();
        }
        Ok(result)
    }

    /// The shares this machine is granting, with what the service says of
    /// each person, and a grant made to each who has accepted. With
    /// `revoke`, the invitation `invitation` of that share is revoked first.
    /// Without `refresh`, as the background pass that keeps granting while
    /// the viewer runs: only unfinished shares, at most eight, taking turns.
    pub async fn named_shares(
        &self,
        revoke: &str,
        invitation: &str,
        refresh: bool,
    ) -> Result<Vec<Shared>, Error> {
        let server = self.server()?;
        if !revoke.is_empty() && !share_id(revoke) {
            return Err(refused("invalid share id"));
        }
        if !revoke.is_empty() && invitation.is_empty() {
            return Err(refused("invalid invitation id"));
        }
        // Nothing to ask the service when this machine never shared with
        // anyone, unless a revocation was asked for: that must reach it.
        if revoke.is_empty() && !self.home.join("named-shares").exists() {
            return Ok(Vec::new());
        }
        self.signed_in()?;
        let _held = self.exclusive(BUSY).await?;
        let keys = self.keys(server, false).await?;
        if !revoke.is_empty() {
            let body = json!({"action": "revoke", "invite_id": invitation});
            self.named_post(server, revoke, &body).await?;
        }
        let state = &keys.verified.state;
        let mut items = read_outbox(&self.home, server, &state.account, &state.root)?;
        // Written down as soon as the service has it, so this machine never
        // grants the person a key afterwards.
        if !revoke.is_empty()
            && let Some(saved) = items.iter_mut().find(|saved| saved.id == revoke)
            && saved.secrets.contains_key(invitation)
        {
            saved
                .status
                .insert(invitation.to_owned(), "revoked".to_owned());
            save(&outbox_file(&self.home, &saved.sha256), saved)?;
        }
        if !refresh && !items.is_empty() {
            // Each pass starts somewhere else, so shares nobody answers
            // cannot keep the later ones waiting.
            let start = (crate::identity::now() / 1000 / 15 * 8) as usize % items.len();
            items.rotate_left(start);
        }
        let mut results = Vec::new();
        for mut saved in items {
            let finished = saved.status.len() == saved.invitations.len()
                && saved
                    .status
                    .values()
                    .all(|status| matches!(status.as_str(), "ready" | "revoked" | "expired"));
            if finished && !refresh {
                continue;
            }
            if !refresh && results.len() >= BATCH {
                break;
            }
            let outcome = self.grant(server, &mut saved, &keys).await;
            // The statuses known now: a person just granted is shown the
            // link without the secret.
            let mut result = saved.result();
            if let Err(error) = outcome {
                result.error = error.to_string();
            }
            results.push(result);
        }
        Ok(results)
    }

    /// The record, verified, pinned and opened with this device's keys. With
    /// `enable`, an account that does not share yet is given its first
    /// incoming-share key.
    async fn keys(&self, server: &str, enable: bool) -> Result<Keys, Error> {
        let remote = self.request(server, None).await?;
        let Some(record) = remote.record else {
            return Err(refused(
                "set up recovery in Recovery and devices before sharing with people",
            ));
        };
        let verified = verify_record(&record, &remote.account)?;
        let file = device::file(&self.home, server, &remote.account);
        let Some(mut device) = device::load(&file)? else {
            return Err(Error::Locked);
        };
        device.pin(&record, &verified)?;
        device::save(&file, &device)?;
        let own = verified
            .state
            .device(&device.id())
            .cloned()
            .ok_or(Error::Locked)?;
        let key = verified.state.unwrap_key(&device.agreement, &own.wrap)?;
        Contents::open(&key, &verified, server)?;
        if enable && verified.state.inbox.is_none() {
            self.once(server, "sharing", &Input::default()).await?;
            return Box::pin(self.keys(server, false)).await;
        }
        Ok(Keys {
            verified,
            device,
            own,
        })
    }

    /// Asks the service what it holds of `saved` and grants each person who
    /// has accepted, once their claim holds. Each grant is written down as
    /// soon as it is made, so one that fails later cannot undo it.
    async fn grant(&self, server: &str, saved: &mut Outbox, keys: &Keys) -> Result<(), Error> {
        let path = format!("/api/named-shares/{}", saved.id);
        let answer = self.named_request(server, &path, &[], None).await?;
        let remote = serde_json::from_slice::<Value>(&answer)
            .ok()
            .as_ref()
            .and_then(Remote::read)
            .ok_or_else(invalid)?;
        if !remote.owner
            || remote.id != saved.id
            || remote.sha256 != saved.sha256
            || saved.root != keys.verified.state.root
        {
            return Err(refused("private share identity changed"));
        }
        let file = outbox_file(&self.home, &saved.sha256);
        // A claim that does not hold is named and passed over, so the people
        // listed after it still get their keys.
        let mut failed = None;
        for mut listed in remote.recipients {
            let known = saved
                .invitations
                .iter()
                .any(|invitation| invitation.id == listed.id && invitation.email == listed.email);
            if !known {
                return Err(refused("recipient list changed"));
            }
            // What this machine knows stays known: a person revoked here, or
            // granted already, is not granted again whatever the service
            // lists. Go takes the service's word.
            let status = match (saved.status.get(&listed.id), listed.status.as_str()) {
                (Some(known), _) if known == "revoked" => "revoked",
                (_, "revoked") => "revoked",
                (Some(known), _) if known == "ready" => "ready",
                (_, listed) => listed,
            }
            .to_owned();
            saved.status.insert(listed.id.clone(), status.clone());
            let Some((claim, proof)) = listed.claim.take().filter(|_| status == "accepted") else {
                continue;
            };
            let mut refuse = |error: slink_protocol::Error| {
                failed.get_or_insert_with(|| format!("{}: {error}", listed.email));
            };
            let secret = saved.secrets.get(&listed.id).map_or("", String::as_str);
            let secret = decode_exact::<32>(secret)?;
            let offer = Offer {
                share_id: &saved.id,
                invite_id: &listed.id,
                sha256: &saved.sha256,
                owner: &saved.account,
                owner_root: &saved.root,
                secret: &secret,
            };
            // The proof is checked before the person's history is asked
            // for, one person at a time, so that ten people do not make one
            // answer ten times as long.
            if listed.history.is_empty() {
                use slink_protocol::Error as Refusal;
                if let Err(error @ (Refusal::Claim | Refusal::ClaimProof)) =
                    verify_claim(&claim, &proof, &offer, &[])
                {
                    refuse(error);
                    continue;
                }
                let path = format!("/api/named-shares/{}?invite={}", saved.id, listed.id);
                let answer = self.named_request(server, &path, &[], None).await?;
                let proof = serde_json::from_slice::<Value>(&answer)
                    .ok()
                    .as_ref()
                    .and_then(Remote::read)
                    .ok_or_else(invalid)?;
                listed.history = proof
                    .recipients
                    .into_iter()
                    .find(|item| item.id == listed.id)
                    .map(|item| item.history)
                    .unwrap_or_default();
            }
            let accepted = match verify_claim(&claim, &proof, &offer, &listed.history) {
                Ok(accepted) => accepted,
                Err(error) => {
                    refuse(error);
                    continue;
                }
            };
            let key = decode_exact::<32>(&saved.key)?;
            let grant = accepted.grant(
                &offer,
                &keys.verified.head,
                &keys.own.id,
                &key,
                &AgreementKey::from_bytes(random()?),
                random()?,
            )?;
            let statement = Signed::sign(Kind::NamedGrant, &grant.encode(), &keys.device.sign)?;
            let body = json!({"action": "grant", "grant": signed_json(&statement)});
            self.named_post(server, &saved.id, &body).await?;
            saved.status.insert(listed.id.clone(), "ready".to_owned());
            save(&file, saved)?;
        }
        save(&file, saved)?;
        failed.map_or(Ok(()), |message| Err(refused(&message)))
    }

    async fn named_post(&self, server: &str, id: &str, body: &Value) -> Result<(), Error> {
        let headers = [("content-type", "application/json".to_owned())];
        let body = slink_format::encode(body).into_bytes();
        let path = format!("/api/named-shares/{id}");
        self.named_request(server, &path, &headers, Some(body))
            .await
            .map(drop)
    }

    /// A request about named shares: a GET, or a POST of `body`. The answer
    /// is its body when the service took it, and the service's message
    /// otherwise.
    async fn named_request(
        &self,
        server: &str,
        path: &str,
        headers: &[(&str, String)],
        body: Option<Vec<u8>>,
    ) -> Result<Vec<u8>, Error> {
        let unreachable =
            || refused("cannot reach private sharing; your pending keys are saved locally");
        let http = client(Duration::from_secs(5 * 60)).map_err(|_| unreachable())?;
        let url = format!("{server}{path}");
        let mut builder = match body {
            Some(body) => http.post(url).body(body),
            None => http.get(url),
        };
        builder = builder.header("authorization", format!("Bearer {}", self.api_key));
        for (name, value) in headers {
            builder = builder.header(*name, value);
        }
        let response = builder.send().await.map_err(|_| unreachable())?;
        let status = response.status().as_u16();
        let raw = read(response, LIMIT).await.ok_or_else(invalid)?;
        if !(200..300).contains(&status) {
            let message = serde_json::from_slice::<Value>(&raw)
                .ok()
                .and_then(|value| value["error"]["message"].as_str().map(str::to_owned))
                .unwrap_or_default();
            return Err(Error::Service {
                status,
                message: format!("{message} (HTTP {status})"),
            });
        }
        Ok(raw)
    }
}

fn invalid() -> Error {
    refused("invalid private-sharing response")
}

/// A share the service has deleted can never be uploaded again: its pending
/// copy goes, so publishing again seals the text afresh. Any other failure
/// keeps it, to be sent again as it is.
fn discard_deleted(pending: &Path, error: Error) -> Error {
    if !matches!(error, Error::Service { status: 410, .. }) {
        return error;
    }
    if let Err(error) = fs::remove_file(pending)
        && error.kind() != io::ErrorKind::NotFound
    {
        return io_error(error);
    }
    Error::Service {
        status: 410,
        message: "this private share was deleted; publish again to create a new one".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn addresses_are_held_to_gos_pattern() {
        for good in [
            "a@b.cd",
            "first.last+tag@mail.example.test",
            "x@1-2.io",
            "o'neil@example.org",
        ] {
            assert!(email_address(good), "{good}");
        }
        for bad in [
            "",
            "@b.cd",
            "a@b",
            "a@.cd",
            "a@b-.cd",
            "a@-b.cd",
            "a@b.c",
            "a@b.c0",
            "A@b.cd",
            "a@B.cd",
            "a b@c.de",
            "a@b@c.de",
            &format!("a@b.{}", "c".repeat(64)),
        ] {
            assert!(!email_address(bad), "{bad}");
        }
    }

    #[test]
    fn recipients_are_trimmed_lowered_and_counted_as_typed() {
        let typed = |list: &[&str]| -> Vec<String> { list.iter().map(|&s| s.to_owned()).collect() };
        assert_eq!(
            recipients(&typed(&[
                " B@Example.test ",
                "a@example.test",
                "b@example.test"
            ]))
            .unwrap(),
            ["a@example.test", "b@example.test"]
        );
        assert_eq!(
            recipients(&[]).unwrap_err().to_string(),
            "choose between one and ten email recipients"
        );
        // Eleven typed is too many, even when some are the same.
        assert!(recipients(&typed(&["a@example.test"; 11])).is_err());
        assert_eq!(
            recipients(&typed(&["a@example.test", "not an address"]))
                .unwrap_err()
                .to_string(),
            "enter a valid email address for each recipient"
        );
    }

    /// A share Go would keep, with fields it does not read.
    fn item() -> Value {
        let secret = [5; 32];
        let id = encode(&[6; 24]);
        json!({
            "server": "https://h", "account": "usr_a", "root": "r", "id": "23456789abcdef",
            "sha256": "A".repeat(64), "key": encode(&[7; 32]), "envelope": "", "extra": 1,
            "invitations": [{"id": id, "email": "b@c.de", "commitment": digest(&secret), "note": "x"}],
            "secrets": {id.clone(): encode(&secret)}, "status": {},
        })
    }

    #[test]
    fn a_share_is_checked_and_kept_with_gos_fields() {
        let good = Outbox::read(&item()).unwrap();
        assert!(good.valid("https://h", "usr_a", "r"));
        assert!(!good.valid("https://h", "usr_b", "r"));
        assert!(!good.valid("https://h", "usr_a", "other"));
        assert!(!good.valid("https://other", "usr_a", "r"));
        let text = good.encode();
        assert!(!text.contains("extra") && !text.contains("note"), "{text}");
        assert!(
            !text.contains("status") && !text.contains("envelope"),
            "{text}"
        );
        assert_eq!(
            Outbox::read(&serde_json::from_str(&text).unwrap()),
            Some(good.clone())
        );
        let broken = [
            Outbox {
                envelope: vec![1],
                ..good.clone()
            },
            Outbox {
                secrets: BTreeMap::new(),
                ..good.clone()
            },
            Outbox {
                invitations: Vec::new(),
                ..good.clone()
            },
            Outbox {
                id: "23456789abcde1".into(),
                ..good.clone()
            },
            Outbox {
                key: "short".into(),
                ..good.clone()
            },
            Outbox {
                sha256: "g".repeat(64),
                ..good.clone()
            },
            Outbox {
                invitations: vec![good.invitations[0].clone(), good.invitations[0].clone()],
                ..good.clone()
            },
        ];
        for bad in broken {
            assert!(!bad.valid("https://h", "usr_a", "r"), "{bad:?}");
        }
        // Types Go cannot read are not read.
        for (field, value) in [
            ("status", json!({"x": 1})),
            ("secrets", json!([])),
            ("invitations", json!({})),
            ("invitations", json!([1])),
            ("envelope", json!("not base64!")),
            ("id", json!(1)),
        ] {
            let mut bad = item();
            bad[field] = value;
            assert_eq!(Outbox::read(&bad), None, "{field}");
        }
    }

    #[test]
    fn a_link_keeps_its_secret_only_while_its_person_is_still_to_accept() {
        let mut saved = Outbox::read(&item()).unwrap();
        let id = saved.invitations[0].id.clone();
        let waiting = &saved.result().recipients[0];
        assert_eq!(waiting.status, "waiting");
        let secret = &saved.secrets[&id];
        assert_eq!(
            waiting.url,
            format!("https://h/n/23456789abcdef?invite={id}#invite={secret}&sender=r")
        );
        for status in ["accepted", "ready", "expired", "revoked"] {
            saved.status.insert(id.clone(), status.to_owned());
            let shown = &saved.result().recipients[0];
            assert_eq!(shown.status, status);
            assert_eq!(shown.url, format!("https://h/n/23456789abcdef?invite={id}"));
        }
        let mut result = saved.result();
        assert!(result.to_json().get("error").is_none());
        result.error = "x".into();
        assert_eq!(result.to_json()["error"], "x");
    }

    #[test]
    fn a_deleted_share_drops_its_pending_copy() {
        let dir = std::env::temp_dir().join(format!("slink-named-pending-{}", std::process::id()));
        let pending = dir.join("pending-fixture.json");
        let write = || write_private(&pending, b"{}").unwrap();
        write();
        // Other failures keep it, to be sent again as it is.
        for error in [
            refused("cannot reach private sharing"),
            Error::Service {
                status: 503,
                message: "busy".into(),
            },
        ] {
            assert_eq!(discard_deleted(&pending, error.clone()), error);
            assert!(pending.exists());
        }
        let deleted = || Error::Service {
            status: 410,
            message: "This share was deleted (HTTP 410)".into(),
        };
        let said = discard_deleted(&pending, deleted());
        assert!(!pending.exists());
        assert!(matches!(said, Error::Service { status: 410, .. }));
        assert!(said.to_string().contains("publish again"), "{said}");
        // Already gone, it still says the share was deleted.
        assert!(matches!(
            discard_deleted(&pending, deleted()),
            Error::Service { status: 410, .. }
        ));
        let _ = fs::remove_dir_all(dir);
    }
}
