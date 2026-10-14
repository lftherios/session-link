//! Sharing with named people against a stand-in for the hosted service that
//! keeps the sender's account as the identity store does, and its share as
//! the named store does: each invitation and each grant must be signed by a
//! device the sender's history holds, at its current head, and a grant must
//! answer the person's claim with the key their account has now. The person
//! who accepts is made here, as a browser would make them. Go's tests of the
//! same, in go/internal/identity/named_test.go, are here too.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use axum::body::to_bytes;
use axum::extract::Request as HttpRequest;
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use slink_client::Identity;
use slink_client::identity::Input;
use slink_client::named::Shared;
use slink_protocol::encoding::{decode_exact, encode};
use slink_protocol::{
    AgreementKey, Claim, Kind, Operation, Record, Recovery, Signed, SigningKey, State,
    binding_proof, device_id, named_context, unwrap, verify_history, verify_record, wrap,
};

const OWNER: &str = "usr_owner";
const KEY: &str = "rk_owner";
const READER: &str = "usr_reader";
const SHARE: &str = "k7qm2xrw9tbn4c";
const TEXT: &str = r#"{"schema":"session/v0","created_at":"2026-09-12T00:00:00Z","name":"PRIVATE_NAMED_SENTINEL","spans":[{"id":"root","type":"agent"}]}"#;

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("slink-named-{name}-{}", std::process::id()));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

fn signed(value: &Value) -> Signed {
    Signed {
        payload: value["payload"].as_str().unwrap().to_owned(),
        signature: value["signature"].as_str().unwrap().to_owned(),
    }
}

fn signed_json(signed: &Signed) -> Value {
    json!({"payload": signed.payload, "signature": signed.signature})
}

fn payload(signed: &Signed, kind: Kind) -> Value {
    serde_json::from_slice(&signed.read(kind).unwrap()).unwrap()
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// One person a share is for, as the service keeps them.
#[derive(Clone)]
struct Person {
    id: String,
    email: String,
    commitment: String,
    claim: Option<(Signed, String)>,
    grant: Option<Signed>,
    revoked: bool,
}

impl Person {
    fn status(&self) -> &'static str {
        match self {
            Person { revoked: true, .. } => "revoked",
            Person { grant: Some(_), .. } => "ready",
            Person { claim: Some(_), .. } => "accepted",
            _ => "waiting",
        }
    }
}

struct Share {
    sha256: String,
    envelope: Vec<u8>,
    people: Vec<Person>,
}

/// What the stand-in keeps, and how it misbehaves when asked to.
#[derive(Default)]
struct Store {
    /// The sender's record.
    record: Option<Record>,
    share: Option<Share>,
    /// Each other account's device history.
    histories: BTreeMap<String, Vec<Signed>>,
    /// Every request: its method, its path with the query, its body.
    sent: Vec<(String, String, Vec<u8>)>,
    /// Uploads are answered as for a share the service deleted.
    deleted: bool,
    /// An upload is taken and its answer lost.
    lose_answer: bool,
    /// The share lists, after its people, one the sender never invited.
    stranger: bool,
}

type Kept = Arc<Mutex<Store>>;

fn reply(status: u16, body: Value) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .header("content-type", "application/json")
        .body(body.to_string().into())
        .unwrap()
}

type Refusal = (u16, String);

fn refuse<T>(status: u16, message: &str) -> Result<T, Refusal> {
    Err((status, message.to_owned()))
}

/// A write to the sender's record, taken only against what it holds.
fn write(store: &mut Store, body: &Value) -> Result<(), Refusal> {
    let current = store
        .record
        .as_ref()
        .map(|record| verify_record(record, OWNER).unwrap());
    let (head, revision) = current.as_ref().map_or((String::new(), 0), |held| {
        (held.head.clone(), held.vault.revision)
    });
    if body["expected_head"] != head || body["expected_revision"] != revision {
        return refuse(409, "Vault changed; refresh and retry");
    }
    let mut events = store
        .record
        .as_ref()
        .map(|record| record.events.clone())
        .unwrap_or_default();
    events.extend(body.get("event").map(signed));
    let next = Record {
        events,
        vault: signed(&body["vault"]),
    };
    let checked = verify_record(&next, OWNER).map_err(|error| (422, error.to_string()))?;
    if checked.vault.revision != revision + 1 {
        return refuse(409, "Invalid vault revision");
    }
    store.record = Some(next);
    Ok(())
}

/// A statement of `kind` from the sender, signed by a device its history
/// holds now, at its current head: the service's rule for invitations and
/// grants.
fn active(store: &Store, kind: Kind, statement: &Signed) -> Result<Value, Refusal> {
    let Some(held) = store
        .record
        .as_ref()
        .map(|record| verify_record(record, OWNER).unwrap())
    else {
        return refuse(409, "Set up recovery and private sharing first");
    };
    let value = payload(statement, kind);
    let device = value["signer"]
        .as_str()
        .and_then(|signer| held.state.device(signer));
    let current = value["account"] == OWNER
        && value["root"] == held.state.root.as_str()
        && value["head"] == held.head.as_str();
    let Some(device) = device.filter(|_| current) else {
        return refuse(409, "Refresh approved device keys and retry");
    };
    if statement.verify(kind, &device.sign_key).is_err() {
        return refuse(422, "Invalid signature");
    }
    if kind == Kind::NamedInvite
        && (held.state.inbox.as_deref() != value["inbox"].as_str()
            || value["epoch"] != held.state.epoch)
    {
        return refuse(422, "Invalid named-sharing invitation");
    }
    Ok(value)
}

fn upload(store: &mut Store, length: usize, body: &[u8]) -> Result<Value, Refusal> {
    if store.deleted {
        return refuse(410, "This private share was deleted");
    }
    let (prefix, envelope) = body.split_at(length);
    let statements: Vec<Value> = serde_json::from_slice(prefix).unwrap();
    let sha256 = hex(envelope);
    let mut people = Vec::new();
    for statement in &statements {
        let value = active(store, Kind::NamedInvite, &signed(statement))?;
        if value["sha256"] != sha256.as_str() {
            return refuse(422, "Invalid named-sharing invitation");
        }
        let invitation = &value["invitation"];
        let text = |name: &str| invitation[name].as_str().unwrap().to_owned();
        people.push(Person {
            id: text("id"),
            email: text("email"),
            commitment: text("commitment"),
            claim: None,
            grant: None,
            revoked: false,
        });
    }
    if let Some(share) = &store.share {
        // A retry is the same ciphertext for the same people.
        let same = |a: &Person, b: &Person| {
            (&a.id, &a.email, &a.commitment) == (&b.id, &b.email, &b.commitment)
        };
        if share.sha256 != sha256
            || share.people.len() != people.len()
            || !share.people.iter().zip(&people).all(|(a, b)| same(a, b))
        {
            return refuse(
                409,
                "This ciphertext already has a different sharing policy",
            );
        }
    } else {
        store.share = Some(Share {
            sha256: sha256.clone(),
            envelope: envelope.to_vec(),
            people,
        });
    }
    Ok(json!({"id": SHARE, "sha256": sha256, "account_id": OWNER}))
}

fn listing(store: &Store, invite: Option<&str>) -> Value {
    let share = store.share.as_ref().unwrap();
    let mut recipients: Vec<Value> = share
        .people
        .iter()
        .map(|person| {
            let mut listed =
                json!({"id": person.id, "email": person.email, "status": person.status()});
            if let Some((claim, proof)) = &person.claim {
                listed["claim"] =
                    json!({"payload": claim.payload, "signature": claim.signature, "proof": proof});
                if invite == Some(person.id.as_str()) && person.grant.is_none() && !person.revoked {
                    let account = payload(claim, Kind::RecipientClaim)["account"]
                        .as_str()
                        .unwrap()
                        .to_owned();
                    let history = store.histories.get(&account).cloned().unwrap_or_default();
                    listed["recipient_history"] =
                        Value::Array(history.iter().map(signed_json).collect());
                }
            }
            listed
        })
        .collect();
    if store.stranger {
        recipients.push(
            json!({"id": encode(&[9; 24]), "email": "stranger@example.test", "status": "waiting"}),
        );
    }
    json!({"id": SHARE, "owner": true, "sha256": share.sha256, "recipients": recipients})
}

fn grant(store: &mut Store, statement: &Signed) -> Result<(), Refusal> {
    let value = active(store, Kind::NamedGrant, statement)?;
    let share = store.share.as_ref().unwrap();
    let Some(person) = share
        .people
        .iter()
        .find(|person| value["invite_id"] == person.id.as_str() && !person.revoked)
    else {
        return refuse(422, "Invalid recipient grant");
    };
    let Some((claim, _)) = &person.claim else {
        return refuse(422, "Invalid recipient grant");
    };
    let bound = payload(claim, Kind::RecipientClaim);
    if value["recipient"] != bound["account"]
        || value["share_id"] != SHARE
        || value["sha256"] != share.sha256.as_str()
        || value["claim_hash"] != claim.hash(Kind::RecipientClaim).unwrap().as_str()
    {
        return refuse(422, "Invalid recipient grant");
    }
    let account = bound["account"].as_str().unwrap();
    let now = verify_history(&store.histories[account], account)
        .unwrap()
        .state;
    if value["recipient_root"] != bound["root"]
        || value["recipient_root"] != now.root.as_str()
        || value["inbox"].as_str() != now.inbox.as_deref()
        || value["epoch"] != now.epoch
    {
        return refuse(409, "Recipient keys changed; wrap with the current key");
    }
    let person = store
        .share
        .as_mut()
        .unwrap()
        .people
        .iter_mut()
        .find(|person| value["invite_id"] == person.id.as_str())
        .unwrap();
    match &person.grant {
        Some(granted) if granted.payload != statement.payload => {
            refuse(409, "Access was already granted")
        }
        _ => {
            person.grant = Some(statement.clone());
            Ok(())
        }
    }
}

/// A service on loopback that keeps the account [`OWNER`] and its share.
async fn service() -> (String, Kept) {
    let store = Kept::default();
    let kept = store.clone();
    let app = axum::Router::new().fallback(move |request: HttpRequest| {
        let store = kept.clone();
        async move {
            let (parts, body) = request.into_parts();
            assert_eq!(parts.headers["authorization"], format!("Bearer {KEY}"));
            let length: usize = parts
                .headers
                .get("x-slink-invitations")
                .and_then(|value| value.to_str().ok()?.parse().ok())
                .unwrap_or_default();
            let body = to_bytes(body, 64 * 1024 * 1024).await.unwrap().to_vec();
            let path = parts.uri.path().to_owned();
            let invite = parts
                .uri
                .query()
                .and_then(|query| query.strip_prefix("invite="));
            let mut store = store.lock().unwrap();
            store.sent.push((
                parts.method.to_string(),
                parts.uri.to_string(),
                body.clone(),
            ));
            let post = parts.method == Method::POST;
            let outcome = match path.as_str() {
                "/api/identity" => if post {
                    let body: Value = serde_json::from_slice(&body).unwrap();
                    write(&mut store, &body)
                } else {
                    Ok(())
                }
                .map(|()| {
                    let record = store.record.as_ref().map_or(Value::Null, |record| {
                        json!({
                            "events": record.events.iter().map(signed_json).collect::<Vec<_>>(),
                            "vault": signed_json(&record.vault),
                        })
                    });
                    json!({"account": OWNER, "record": record, "requests": []})
                }),
                "/api/named-shares" => {
                    let answer = upload(&mut store, length, &body);
                    if answer.is_ok() && store.lose_answer {
                        return reply(502, json!({}));
                    }
                    answer
                }
                "/api/named-shares/k7qm2xrw9tbn4c" if post => {
                    let action: Value = serde_json::from_slice(&body).unwrap();
                    match action["action"].as_str() {
                        Some("grant") => grant(&mut store, &signed(&action["grant"])),
                        Some("revoke") => {
                            let share = store.share.as_mut().unwrap();
                            match share
                                .people
                                .iter_mut()
                                .find(|person| action["invite_id"] == person.id.as_str())
                            {
                                Some(person) => {
                                    person.revoked = true;
                                    Ok(())
                                }
                                None => refuse(404, "Recipient not found"),
                            }
                        }
                        _ => refuse(400, "Unknown sharing action"),
                    }
                    .map(|()| json!({"ok": true}))
                }
                "/api/named-shares/k7qm2xrw9tbn4c" if store.share.is_some() => {
                    Ok(listing(&store, invite))
                }
                _ => refuse(404, "Share not found"),
            };
            match outcome {
                Ok(answer) => reply(200, answer),
                Err((status, message)) => reply(status, json!({"error": {"message": message}})),
            }
        }
    });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{address}"), store)
}

static COUNTER: std::sync::atomic::AtomicU8 = std::sync::atomic::AtomicU8::new(1);

fn random() -> [u8; 32] {
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(29) ^ (std::process::id() as u8);
    }
    bytes[0] ^= COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    bytes
}

/// A person with an account of their own, one device and sharing on, as a
/// browser sets them up.
struct Reader {
    device: SigningKey,
    device_box: AgreementKey,
    inbox: AgreementKey,
    events: Vec<Signed>,
    state: State,
}

fn reader(account: &str) -> Reader {
    let recovery = Recovery {
        root: SigningKey::from_seed(random()),
        agreement: AgreementKey::from_bytes(random()),
    };
    let (device, device_box) = (
        SigningKey::from_seed(random()),
        AgreementKey::from_bytes(random()),
    );
    let inbox = AgreementKey::from_bytes(random());
    let key = random();
    let mut state = State {
        account: account.to_owned(),
        seq: 0,
        prev: String::new(),
        kind: Operation::Init,
        signer: "root".to_owned(),
        root: recovery.root.public(),
        recovery: recovery.agreement.public(),
        recovery_box: recovery.seal(account, &random(), [1; 12]),
        recovery_wrap: String::new(),
        epoch: 1,
        devices: Vec::new(),
        inbox: Some(inbox.public()),
    };
    let context = state.context();
    let wrapped = |to: &str| {
        wrap(
            to,
            &key,
            &context,
            &AgreementKey::from_bytes(random()),
            [2; 12],
        )
    };
    state.recovery_wrap = wrapped(&state.recovery).unwrap();
    state.devices.push(slink_protocol::Device {
        id: device_id(&device.public(), &device_box.public()),
        name: "Reader browser".to_owned(),
        sign_key: device.public(),
        box_key: device_box.public(),
        wrap: wrapped(&device_box.public()).unwrap(),
    });
    let events = vec![Signed::sign(Kind::Event, &state.encode(), &recovery.root).unwrap()];
    let state = verify_history(&events, account).unwrap().state;
    Reader {
        device,
        device_box,
        inbox,
        events,
        state,
    }
}

/// The secret and the sender's root key from an invitation link's fragment.
fn fragment(link: &str) -> ([u8; 32], String) {
    let fragment = link.split_once('#').unwrap().1;
    let secret = fragment
        .split_once("invite=")
        .unwrap()
        .1
        .split('&')
        .next()
        .unwrap();
    let sender = fragment.split_once("&sender=").unwrap().1;
    (decode_exact(secret).unwrap(), sender.to_owned())
}

impl Reader {
    /// The claim a browser makes when its person accepts the invitation
    /// `link`, signed by `signer`, with a proof made with `secret`.
    fn claim(
        &self,
        link: &str,
        signer: &SigningKey,
        secret: &[u8; 32],
        store: &Store,
    ) -> (Signed, String) {
        let share = store.share.as_ref().unwrap();
        let (_, sender) = fragment(link);
        let invite = link
            .split_once("?invite=")
            .unwrap()
            .1
            .split('#')
            .next()
            .unwrap();
        let claim = Claim {
            account: self.state.account.clone(),
            root: self.state.root.clone(),
            head: self.events.last().unwrap().hash(Kind::Event).unwrap(),
            signer: device_id(&signer.public(), &self.device_box.public()),
            share_id: SHARE.to_owned(),
            invite_id: invite.to_owned(),
            sha256: share.sha256.clone(),
            owner: OWNER.to_owned(),
            owner_root: sender,
            inbox: self.state.inbox.clone().unwrap(),
            epoch: self.state.epoch,
        };
        let signed = Signed::sign(Kind::RecipientClaim, &claim.encode(), signer).unwrap();
        let proof = binding_proof(secret, &signed).unwrap();
        (signed, proof)
    }

    /// The share's text, opened with the key the grant wraps to this
    /// account's incoming-share key.
    fn open(&self, store: &Store, invite: &str) -> Vec<u8> {
        let share = store.share.as_ref().unwrap();
        let person = share
            .people
            .iter()
            .find(|person| person.id == invite)
            .unwrap();
        let grant = payload(person.grant.as_ref().unwrap(), Kind::NamedGrant);
        let context = named_context(
            &share.sha256,
            &self.state.account,
            &self.state.root,
            self.state.epoch,
        );
        let key = unwrap(&self.inbox, grant["wrap"].as_str().unwrap(), &context).unwrap();
        slink_protocol::envelope::decrypt(&share.envelope, &encode(&key)).unwrap()
    }
}

/// The person `invite` accepts with `claim`, as the service records it.
fn accept(store: &Kept, invite: &str, claim: (Signed, String), reader: &Reader) {
    let mut store = store.lock().unwrap();
    store
        .histories
        .insert(reader.state.account.clone(), reader.events.clone());
    let person = store
        .share
        .as_mut()
        .unwrap()
        .people
        .iter_mut()
        .find(|person| person.id == invite)
        .unwrap();
    person.claim = Some(claim);
}

/// A sender with recovery set up on this machine, and the recovery key it
/// was shown.
async fn sender(home: &Path, server: &str) -> (Identity, String) {
    let identity = Identity::new(home, server, KEY);
    let input = Input {
        name: "Laptop".to_owned(),
        ..Input::default()
    };
    let status = identity.handle("setup", &input).await.unwrap();
    assert_eq!(status.state, "ready");
    identity
        .handle("confirm-recovery", &Input::default())
        .await
        .unwrap();
    (identity, status.recovery_key)
}

fn people(list: &[&str]) -> Vec<String> {
    list.iter().map(|&email| email.to_owned()).collect()
}

/// The requests whose path starts with `path` and whose body holds `marker`.
fn count(store: &Kept, path: &str, marker: &str) -> usize {
    let store = store.lock().unwrap();
    store
        .sent
        .iter()
        .filter(|(_, sent, body)| {
            sent.starts_with(path) && String::from_utf8_lossy(body).contains(marker)
        })
        .count()
}

fn grants(store: &Kept) -> usize {
    count(store, "/api/named-shares/k7qm2xrw9tbn4c", "\"grant\"")
}

fn person<'a>(shared: &'a Shared, email: &str) -> &'a slink_client::named::Recipient {
    shared
        .recipients
        .iter()
        .find(|person| person.email == email)
        .unwrap()
}

/// The one file in `home`'s outbox, read.
fn outbox(home: &Path) -> (PathBuf, Value) {
    let files: Vec<PathBuf> = fs::read_dir(home.join("named-shares"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .collect();
    assert_eq!(files.len(), 1, "{files:?}");
    let saved = serde_json::from_slice(&fs::read(&files[0]).unwrap()).unwrap();
    (files[0].clone(), saved)
}

#[tokio::test]
async fn a_share_reaches_only_the_people_invited_and_each_gets_the_key() {
    let (server, store) = service().await;
    let (a, b) = (home("journey-a"), home("journey-b"));
    let (owner, recovery_key) = sender(&a, &server).await;

    let shared = owner
        .publish_named(
            TEXT,
            &people(&["Reader@Example.test ", "other@example.test"]),
        )
        .await
        .unwrap();
    assert_eq!(shared.url, format!("{server}/n/{SHARE}"));
    assert_eq!(shared.error, "", "the vault holds the share");
    let emails: Vec<&str> = shared.recipients.iter().map(|p| p.email.as_str()).collect();
    assert_eq!(emails, ["other@example.test", "reader@example.test"]);
    let held = verify_record(store.lock().unwrap().record.as_ref().unwrap(), OWNER).unwrap();
    assert!(held.state.inbox.is_some(), "sharing was turned on");
    let invitation = person(&shared, "reader@example.test").clone();
    let (secret, root) = fragment(&invitation.url);
    assert_eq!(root, held.state.root);
    assert_eq!(invitation.status, "waiting");
    let (file, saved) = outbox(&a);
    assert!(
        saved.get("envelope").is_none(),
        "the ciphertext goes once acknowledged"
    );

    // Each statement names one person; none says who else was invited.
    {
        let store = store.lock().unwrap();
        let (_, _, upload) = store
            .sent
            .iter()
            .find(|(method, path, _)| method == "POST" && path == "/api/named-shares")
            .unwrap();
        let mut values = serde_json::Deserializer::from_slice(upload).into_iter::<Value>();
        let statements = values.next().unwrap().unwrap();
        assert_eq!(statements.as_array().unwrap().len(), 2);
        for statement in statements.as_array().unwrap() {
            let text = payload(&signed(statement), Kind::NamedInvite).to_string();
            let named = ["reader@example.test", "other@example.test"]
                .iter()
                .filter(|email| text.contains(*email))
                .count();
            assert_eq!(named, 1, "{text}");
        }
    }

    let reader = reader(READER);
    let claim = reader.claim(
        &invitation.url,
        &reader.device,
        &secret,
        &store.lock().unwrap(),
    );
    accept(&store, &invitation.id, claim, &reader);

    // Another of the account's machines, recovered with the key, has the
    // share from the vault and goes on granting it.
    let second = Identity::new(&b, &server, KEY);
    let input = Input {
        name: "Desktop".to_owned(),
        recovery_key,
        ..Input::default()
    };
    assert_eq!(
        second.handle("recover", &input).await.unwrap().state,
        "ready"
    );
    let copied = b.join("named-shares").join(file.file_name().unwrap());
    assert!(
        copied.exists(),
        "the vault's share was written out on the new machine"
    );
    let listed = second.named_shares("", "", true).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].error, "");
    let ready = person(&listed[0], "reader@example.test");
    assert_eq!(ready.status, "ready");
    assert_eq!(
        ready.url,
        format!("{server}/n/{SHARE}?invite={}", invitation.id)
    );
    let waiting = person(&listed[0], "other@example.test");
    assert_eq!(waiting.status, "waiting");
    assert!(waiting.url.contains("#invite="));
    assert_eq!(grants(&store), 1);
    assert_eq!(
        reader.open(&store.lock().unwrap(), &invitation.id),
        TEXT.as_bytes()
    );

    // The first machine's background pass sees the grant and makes no
    // second one.
    let again = owner.named_shares("", "", false).await.unwrap();
    assert_eq!(again.len(), 1);
    assert_eq!(person(&again[0], "reader@example.test").status, "ready");
    assert_eq!(grants(&store), 1);

    // Revoked, the person's status says so, and with everyone finished the
    // background pass has nothing to look at.
    let other = person(&again[0], "other@example.test").id.clone();
    let revoked = owner.named_shares(SHARE, &other, true).await.unwrap();
    assert_eq!(person(&revoked[0], "other@example.test").status, "revoked");
    assert!(owner.named_shares("", "", false).await.unwrap().is_empty());

    // No key, secret or text reached the service.
    let mut secrets = vec![
        saved["key"].as_str().unwrap().to_owned(),
        "PRIVATE_NAMED_SENTINEL".to_owned(),
    ];
    secrets.extend(
        saved["secrets"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| v.as_str().unwrap().to_owned()),
    );
    let store = store.lock().unwrap();
    for (_, path, body) in &store.sent {
        let body = String::from_utf8_lossy(body);
        for secret in &secrets {
            assert!(
                !body.contains(secret.as_str()) && !path.contains(secret.as_str()),
                "{path}"
            );
        }
    }
}

/// Go's TestNamedGrantRequiresInvitationProofAndApprovedHistory: no key
/// leaves this machine for a claim without the invitation's secret, from a
/// device the person's history never approved, or for another invitation,
/// and a claimed invitation's link drops its secret whatever happens next.
#[tokio::test]
async fn a_claim_that_does_not_hold_gets_no_key() {
    let (server, store) = service().await;
    let a = home("forged");
    let (owner, _) = sender(&a, &server).await;
    let shared = owner
        .publish_named(
            TEXT,
            &people(&["reader@example.test", "other@example.test"]),
        )
        .await
        .unwrap();
    let invitation = person(&shared, "reader@example.test").clone();
    let other = person(&shared, "other@example.test").clone();
    let (secret, _) = fragment(&invitation.url);
    let (other_secret, _) = fragment(&other.url);
    let reader = reader(READER);
    let canonical = format!("{server}/n/{SHARE}?invite={}", invitation.id);
    let stranger_device = SigningKey::from_seed(random());

    let valid = reader.claim(
        &invitation.url,
        &reader.device,
        &secret,
        &store.lock().unwrap(),
    );
    let forgeries: Vec<(&str, (Signed, String))> = vec![
        // The service's own account, which never saw the secret.
        (
            "a proof made without the secret",
            (valid.0.clone(), encode(&random())),
        ),
        ("another invitation's secret", {
            let made = reader.claim(
                &invitation.url,
                &reader.device,
                &other_secret,
                &store.lock().unwrap(),
            );
            (made.0, made.1)
        }),
        ("a signature from elsewhere", {
            let mut claim = valid.0.clone();
            claim.signature = encode(&[7; 64]);
            (claim, valid.1.clone())
        }),
        ("a payload for another account", {
            let mut claim = valid.0.clone();
            claim.payload = encode(br#"{"account":"usr_attacker"}"#);
            (claim, valid.1.clone())
        }),
        ("a device the history never approved", {
            reader.claim(
                &invitation.url,
                &stranger_device,
                &secret,
                &store.lock().unwrap(),
            )
        }),
    ];
    for (case, forged) in forgeries {
        accept(&store, &invitation.id, forged, &reader);
        let listed = owner.named_shares("", "", true).await.unwrap();
        assert_ne!(listed[0].error, "", "{case}");
        let shown = person(&listed[0], "reader@example.test");
        assert_eq!(
            (shown.status.as_str(), shown.url.as_str()),
            ("accepted", canonical.as_str()),
            "{case}"
        );
        assert_eq!(grants(&store), 0, "{case}: a key was released");
    }
    // A proof that does not hold is refused before the person's history is
    // asked for.
    assert_eq!(
        count(&store, &format!("/api/named-shares/{SHARE}?invite="), ""),
        2
    );

    accept(&store, &invitation.id, valid, &reader);
    let listed = owner.named_shares("", "", true).await.unwrap();
    assert_eq!(listed[0].error, "");
    assert_eq!(person(&listed[0], "reader@example.test").status, "ready");
    assert_eq!(grants(&store), 1);
    assert_eq!(
        reader.open(&store.lock().unwrap(), &invitation.id),
        TEXT.as_bytes()
    );

    // With the grant already made, a stale local status makes no second one
    // and shows no secret.
    let (file, mut saved) = outbox(&a);
    saved["status"] = json!({invitation.id.clone(): "accepted"});
    fs::write(&file, saved.to_string()).unwrap();
    let listed = owner.named_shares("", "", true).await.unwrap();
    assert_eq!(person(&listed[0], "reader@example.test").url, canonical);
    assert_eq!(grants(&store), 1);

    // A person the sender never invited stops the pass, after the grant to
    // one who was invited is written down.
    let other_claim = reader.claim(
        &other.url,
        &reader.device,
        &other_secret,
        &store.lock().unwrap(),
    );
    accept(&store, &other.id, other_claim, &reader);
    store.lock().unwrap().stranger = true;
    let listed = owner.named_shares("", "", true).await.unwrap();
    assert_eq!(listed[0].error, "recipient list changed");
    assert_eq!(grants(&store), 2);
    let (_, saved) = outbox(&a);
    assert_eq!(saved["status"][other.id.as_str()], "ready", "{saved}");
}

#[tokio::test]
async fn a_share_whose_answer_was_lost_is_sent_again_as_it_was() {
    let (server, store) = service().await;
    let a = home("lost");
    let (owner, _) = sender(&a, &server).await;
    store.lock().unwrap().lose_answer = true;
    let lost = owner
        .publish_named(
            TEXT,
            &people(&["reader@example.test", "other@example.test"]),
        )
        .await
        .unwrap_err();
    assert_eq!(lost.to_string(), " (HTTP 502)");
    let pending: Vec<_> = fs::read_dir(a.join("named-shares"))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    assert!(
        pending.len() == 1 && pending[0].starts_with("pending-"),
        "{pending:?}"
    );
    store.lock().unwrap().lose_answer = false;
    // The same people, typed otherwise, are the same share.
    let shared = owner
        .publish_named(
            TEXT,
            &people(&[
                "Other@example.test",
                "reader@example.test",
                "reader@example.test",
            ]),
        )
        .await
        .unwrap();
    assert_eq!(shared.id, SHARE);
    let uploads: Vec<Vec<u8>> = store
        .lock()
        .unwrap()
        .sent
        .iter()
        .filter(|(method, path, _)| method == "POST" && path == "/api/named-shares")
        .map(|(_, _, body)| body.clone())
        .collect();
    assert_eq!(uploads.len(), 2);
    let envelope = |body: &[u8]| {
        let mut values = serde_json::Deserializer::from_slice(body).into_iter::<Value>();
        values.next();
        body[values.byte_offset()..].to_vec()
    };
    assert_eq!(
        envelope(&uploads[0]),
        envelope(&uploads[1]),
        "the same ciphertext"
    );
    let (_, saved) = outbox(&a);
    assert_eq!(saved["id"], SHARE);
}

#[tokio::test]
async fn a_deleted_share_is_sealed_afresh_when_published_again() {
    let (server, store) = service().await;
    let a = home("deleted");
    let (owner, _) = sender(&a, &server).await;
    store.lock().unwrap().deleted = true;
    let refused = owner
        .publish_named(TEXT, &people(&["reader@example.test"]))
        .await
        .unwrap_err();
    assert_eq!(
        refused.to_string(),
        "this private share was deleted; publish again to create a new one"
    );
    assert_eq!(fs::read_dir(a.join("named-shares")).unwrap().count(), 0);
    store.lock().unwrap().deleted = false;
    owner
        .publish_named(TEXT, &people(&["reader@example.test"]))
        .await
        .unwrap();
    let hashes: Vec<String> = store
        .lock()
        .unwrap()
        .sent
        .iter()
        .filter(|(method, path, _)| method == "POST" && path == "/api/named-shares")
        .map(|(_, _, body)| {
            let mut values = serde_json::Deserializer::from_slice(body).into_iter::<Value>();
            values.next();
            hex(&body[values.byte_offset()..])
        })
        .collect();
    assert_eq!(hashes.len(), 2);
    assert_ne!(hashes[0], hashes[1]);
}

/// Go's TestRevokeWithoutLocalOutboxIsNotSilentlyDropped.
#[tokio::test]
async fn a_revocation_reaches_the_service_or_fails() {
    let (server, store) = service().await;
    let a = home("revoke");
    let identity = Identity::new(&a, &server, KEY);
    assert!(
        identity
            .named_shares("", "", true)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        identity
            .named_shares("not-a-share", "invite", true)
            .await
            .is_err()
    );
    assert!(identity.named_shares(SHARE, "", true).await.is_err());
    assert!(
        store.lock().unwrap().sent.is_empty(),
        "nothing to ask the service"
    );
    assert!(!a.join("identity").exists(), "nor anything written");
    // With nothing kept here, a revocation still has to reach the service.
    let failed = identity
        .named_shares(SHARE, "invite", true)
        .await
        .unwrap_err();
    assert!(!store.lock().unwrap().sent.is_empty());
    assert_eq!(
        failed.to_string(),
        "set up recovery in Recovery and devices before sharing with people"
    );
    // No key, nothing asked.
    let signed_out = Identity::new(&a, &server, "");
    assert_eq!(
        signed_out
            .named_shares(SHARE, "invite", true)
            .await
            .unwrap_err()
            .status(),
        401
    );
}

/// Go's TestUnverifiableOutboxFilesNeverBlockVaultBackup: a share from an
/// identity the account has since replaced, and a file that is no share,
/// stay on this machine. They are not listed, block no backup, and do not
/// reach another machine through the vault.
#[tokio::test]
async fn shares_that_cannot_be_checked_stay_on_this_machine() {
    let (server, _store) = service().await;
    let (a, b) = (home("unchecked-a"), home("unchecked-b"));
    let (owner, recovery_key) = sender(&a, &server).await;
    owner
        .publish_named(TEXT, &people(&["reader@example.test"]))
        .await
        .unwrap();
    let (file, saved) = outbox(&a);
    let dir = file.parent().unwrap().to_owned();
    let mut previous = saved.clone();
    previous["root"] = json!(encode(&random()));
    previous["sha256"] = json!("b".repeat(64));
    fs::write(
        dir.join(format!("{}.json", "b".repeat(64))),
        previous.to_string(),
    )
    .unwrap();
    fs::write(dir.join(format!("{}.json", "c".repeat(64))), "[1]").unwrap();
    let listed = owner.named_shares("", "", true).await.unwrap();
    assert_eq!(listed.len(), 1, "only this identity's share is listed");
    owner.handle("sync", &Input::default()).await.unwrap();
    let second = Identity::new(&b, &server, KEY);
    let input = Input {
        recovery_key,
        ..Input::default()
    };
    second.handle("recover", &input).await.unwrap();
    let (copied, kept) = outbox(&b);
    assert_eq!(copied.file_name(), file.file_name());
    assert_eq!(kept["sha256"], saved["sha256"]);
}
