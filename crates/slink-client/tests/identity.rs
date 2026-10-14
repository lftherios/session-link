//! Device keys and the vault against a stand-in for the hosted service that
//! keeps one account as the service does: it verifies each write with the
//! protocol crate, takes it only against the head and revision it holds, and
//! keeps approval requests. Go's tests of the same, in
//! go/internal/identity/setup_test.go and identity_test.go, are here too.
//!
//! With `SLINK_IDENTITY_BRIDGE` set to the server checkout's
//! `scripts/identity-bridge.mjs`, the journey runs against the service's own
//! store as well.

use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};

use axum::body::to_bytes;
use axum::extract::Request as HttpRequest;
use axum::http::{Method, StatusCode};
use axum::response::Response;
use serde_json::{Value, json};
use slink_client::Identity;
use slink_client::identity::{Error, Input, Status};
use slink_protocol::encoding::{digest, encode};
use slink_protocol::{
    AgreementKey, Kind, Operation, Record, Recovery, Request, Signed, SigningKey, State, Vault,
    verify_history, verify_record, wrap,
};

const ACCOUNT: &str = "usr_fixture";
const KEY: &str = "rk_fixture";

fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("slink-identity-{name}-{}", std::process::id()));
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

fn record_json(record: &Record) -> Value {
    json!({
        "events": record.events.iter().map(signed_json).collect::<Vec<_>>(),
        "vault": signed_json(&record.vault),
    })
}

/// What the stand-in keeps, and how it misbehaves when asked to.
#[derive(Default)]
struct Store {
    record: Option<Record>,
    requests: Vec<Signed>,
    /// Writes are not taken and no answer comes back.
    offline: bool,
    /// Writes are taken and the answer is lost.
    lose_answer: bool,
    /// A setup is stored as an identity the service made, with the same
    /// first device: a service that swaps the identity out.
    substitute: bool,
    /// Writes taken.
    posts: usize,
    /// Every write sent, taken or not.
    sent: Vec<Value>,
    /// How many writes to refuse as if another device wrote first.
    races: usize,
}

type Shared = Arc<Mutex<Store>>;

fn reply(status: u16, body: Value) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .header("content-type", "application/json")
        .body(body.to_string().into())
        .unwrap()
}

fn refuse(status: u16, message: &str) -> Result<(), (u16, String)> {
    Err((status, message.to_owned()))
}

/// A write as the service's identity store takes it.
fn apply(store: &mut Store, body: &Value) -> Result<(), (u16, String)> {
    if let Some(request) = body.get("request") {
        let Some(record) = &store.record else {
            return refuse(409, "Set up recovery on your first device first");
        };
        let state = verify_record(record, ACCOUNT).unwrap().state;
        let request = signed(request);
        let Ok(read) = Request::verify(&request) else {
            return refuse(422, "Invalid device request");
        };
        if read.account != ACCOUNT || read.root != state.root {
            return refuse(422, "Invalid device request");
        }
        store
            .requests
            .retain(|kept| Request::verify(kept).unwrap().id() != read.id());
        store.requests.push(request);
        return Ok(());
    }
    let current = store
        .record
        .as_ref()
        .map(|record| verify_record(record, ACCOUNT).unwrap());
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
    let event = body.get("event").map(signed);
    events.extend(event.clone());
    let next = Record {
        events,
        vault: signed(&body["vault"]),
    };
    let checked = verify_record(&next, ACCOUNT).map_err(|error| (422, error.to_string()))?;
    if checked.vault.revision != revision + 1 {
        return refuse(409, "Invalid vault revision");
    }
    let writer = if event.is_some() {
        checked.state.signer == checked.vault.signer
    } else {
        checked.vault.signer != "root"
    };
    if !writer {
        return refuse(403, "Invalid vault writer");
    }
    if event.is_some() && checked.state.kind == Operation::Approve {
        let before = &current.as_ref().unwrap().state;
        let added = checked
            .state
            .devices
            .iter()
            .find(|device| before.device(&device.id).is_none())
            .unwrap();
        if !store
            .requests
            .iter()
            .any(|request| Request::verify(request).unwrap().id() == added.id)
        {
            return refuse(409, "Device request expired; start again");
        }
    }
    store.record = Some(if store.substitute && current.is_none() {
        substitute(&event.unwrap())
    } else {
        next
    });
    let devices = verify_record(store.record.as_ref().unwrap(), ACCOUNT)
        .unwrap()
        .state
        .devices;
    store.requests.retain(|request| {
        let id = Request::verify(request).unwrap().id();
        devices.iter().all(|device| device.id != id)
    });
    Ok(())
}

/// A service on loopback that keeps the account [`ACCOUNT`].
async fn service() -> (String, Shared) {
    let store = Shared::default();
    let kept = store.clone();
    let app = axum::Router::new().fallback(move |request: HttpRequest| {
        let store = kept.clone();
        async move {
            let (parts, body) = request.into_parts();
            assert_eq!(parts.uri.path(), "/api/identity");
            assert_eq!(parts.headers["authorization"], format!("Bearer {KEY}"));
            let body = to_bytes(body, 16 * 1024 * 1024).await.unwrap();
            let mut store = store.lock().unwrap();
            if parts.method == Method::POST {
                let body: Value = serde_json::from_slice(&body).unwrap();
                store.sent.push(body.clone());
                if store.offline {
                    return reply(502, json!({}));
                }
                store.posts += 1;
                if store.races > 0 && body.get("vault").is_some() {
                    store.races -= 1;
                    return reply(
                        409,
                        json!({"error": {"message": "Vault changed; refresh and retry"}}),
                    );
                }
                if let Err((status, message)) = apply(&mut store, &body) {
                    return reply(status, json!({"error": {"message": message}}));
                }
                if store.lose_answer {
                    return reply(502, json!({}));
                }
            }
            let record = store.record.as_ref().map_or(Value::Null, record_json);
            let requests: Vec<Value> = store.requests.iter().map(signed_json).collect();
            reply(
                200,
                json!({"account": ACCOUNT, "record": record, "requests": requests}),
            )
        }
    });
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{address}"), store)
}

fn random() -> [u8; 32] {
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(37) ^ (std::process::id() as u8);
    }
    bytes[0] ^= COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) as u8;
    bytes
}

static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

/// A valid identity the service made itself, from what a device sent: a
/// new root and recovery key, a vault key the service chose, wrapped to the
/// device that set up. Taking it would hand the service every key the
/// device backs up next.
fn substitute(event: &Signed) -> Record {
    let sent = verify_history(std::slice::from_ref(event), ACCOUNT)
        .unwrap()
        .state;
    let device = &sent.devices[0];
    forged(&device.sign_key, &device.box_key, &device.name)
}

/// A whole identity, root to vault, whose one device has these keys.
fn forged(sign_key: &str, box_key: &str, name: &str) -> Record {
    let recovery = Recovery {
        root: SigningKey::from_seed(random()),
        agreement: AgreementKey::from_bytes(random()),
    };
    let key = random();
    let mut state = State {
        account: ACCOUNT.to_owned(),
        seq: 0,
        prev: String::new(),
        kind: Operation::Init,
        signer: "root".to_owned(),
        root: recovery.root.public(),
        recovery: recovery.agreement.public(),
        recovery_box: recovery.seal(ACCOUNT, &random(), [1; 12]),
        recovery_wrap: String::new(),
        epoch: 1,
        devices: Vec::new(),
        inbox: None,
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
        id: slink_protocol::device_id(sign_key, box_key),
        name: name.to_owned(),
        sign_key: sign_key.to_owned(),
        box_key: box_key.to_owned(),
        wrap: wrapped(box_key).unwrap(),
    });
    let event = Signed::sign(Kind::Event, &state.encode(), &recovery.root).unwrap();
    let vault = Vault {
        account: ACCOUNT.to_owned(),
        revision: 1,
        head: event.hash(Kind::Event).unwrap(),
        epoch: 1,
        signer: "root".to_owned(),
        data: state.seal_vault(&key, br#"{"version":1,"shares":[]}"#, [3; 12]),
    };
    let record = Record {
        events: vec![event],
        vault: Signed::sign(Kind::Vault, &vault.encode(), &recovery.root).unwrap(),
    };
    verify_record(&record, ACCOUNT).unwrap();
    record
}

/// This machine's device file for the account, as written.
fn device_file(home: &Path, server: &str) -> Option<Value> {
    let name = digest(format!("{server}\0{ACCOUNT}").as_bytes());
    let raw = fs::read(home.join("identity").join(format!("{name}.json"))).ok()?;
    Some(serde_json::from_slice(&raw).unwrap())
}

fn input(name: &str) -> Input {
    Input {
        name: name.to_owned(),
        ..Input::default()
    }
}

async fn call(identity: &Identity, action: &str, input: Input) -> Status {
    match identity.handle(action, &input).await {
        Ok(status) => status,
        Err(error) => panic!("{action}: {error}"),
    }
}

/// A link this machine published, as `publish` keeps its receipt.
fn publish(home: &Path, server: &str, id: &str, account: &str) -> (String, String) {
    let key = encode(&random());
    let url = format!("{server}/s/{id}#key={key}");
    let receipt = json!({"account": account, "server": server, "key": key, "sha256": "a".repeat(64), "url": url});
    fs::create_dir_all(home.join("shares")).unwrap();
    fs::write(
        home.join("shares").join(format!("{id}.json")),
        receipt.to_string(),
    )
    .unwrap();
    (url, key)
}

fn urls(status: &Status) -> Vec<&str> {
    status
        .shares
        .iter()
        .map(|receipt| receipt.url.as_str())
        .collect()
}

/// The record a service at `server` holds, read as a device reads it.
async fn held(server: &str) -> Record {
    let _ = rustls::crypto::ring::default_provider().install_default();
    let answer = reqwest::Client::new()
        .get(format!("{server}/api/identity"))
        .header("authorization", format!("Bearer {KEY}"))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    let answer: Value = serde_json::from_str(&answer).unwrap();
    Record {
        events: answer["record"]["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(signed)
            .collect(),
        vault: signed(&answer["record"]["vault"]),
    }
}

/// Three machines on one account: setup and its recovery key, a link backed
/// up, sharing turned on, a second machine approved by code and revoked, a
/// third recovered with the key, and what each can then read. Returns what
/// no request may carry, for the caller to look for in what was stored.
async fn journey(server: &str, name: &str) -> Vec<String> {
    let (a, b, c) = (
        home(&format!("{name}-a")),
        home(&format!("{name}-b")),
        home(&format!("{name}-c")),
    );
    let (laptop, desktop, spare) = (
        Identity::new(&a, server, KEY),
        Identity::new(&b, server, KEY),
        Identity::new(&c, server, KEY),
    );

    let first = call(&laptop, "setup", input("Laptop")).await;
    assert_eq!(first.state, "ready");
    assert!(first.recovery_pending);
    let token = first.recovery_key.clone();
    assert!(token.starts_with("slr1."), "{token}");
    // Asked again before it is confirmed, the same key.
    assert_eq!(
        call(&laptop, "setup", Input::default()).await.recovery_key,
        token
    );
    let confirmed = call(&laptop, "confirm-recovery", Input::default()).await;
    assert!(!confirmed.recovery_pending && confirmed.recovery_key.is_empty());
    assert!(
        device_file(&a, server)
            .unwrap()
            .get("pending_recovery")
            .is_none()
    );
    let seed = device_file(&a, server).unwrap()["seed"]
        .as_str()
        .unwrap()
        .to_owned();

    let (link, content_key) = publish(&a, server, "23456789abcdef", ACCOUNT);
    // A receipt for another account, or no account, stays on this machine.
    publish(&a, server, "23456789abcdeg", "usr_other");
    publish(&a, server, "23456789abcdeh", "");
    assert_eq!(
        urls(&call(&laptop, "sync", Input::default()).await),
        [link.as_str()]
    );

    // Sharing with named people needs an incoming-share key: one event adds
    // it, and the vault keeps its private half.
    call(&laptop, "sharing", Input::default()).await;
    let shared = verify_record(&held(server).await, ACCOUNT).unwrap();
    let inbox = shared.state.inbox.clone().expect("an incoming-share key");
    assert_eq!(shared.state.kind, Operation::Sharing);
    // Asked again, nothing changes.
    call(&laptop, "sharing", Input::default()).await;
    assert_eq!(
        held(server).await.events.len(),
        shared.state.seq as usize + 1
    );

    // Signed in is not unlocked.
    let locked = call(&desktop, "status", Input::default()).await;
    assert_eq!((locked.state, locked.shares.len()), ("locked", 0));
    assert_eq!(
        desktop.handle("sync", &Input::default()).await,
        Err(Error::Locked)
    );
    assert_eq!(desktop.backup(ACCOUNT).await, "setup");

    let asked = call(&desktop, "request", input("Desktop")).await;
    assert_eq!(asked.confirmation.len(), 14);
    assert_eq!(desktop.backup(ACCOUNT).await, "locked");
    let waiting = call(&laptop, "status", Input::default()).await;
    assert_eq!(waiting.requests.len(), 1);
    assert_eq!(waiting.requests[0].1, "Desktop");
    let wrong = laptop
        .handle(
            "approve",
            &Input {
                id: asked.device_id.clone(),
                code: "WRONG".to_owned(),
                ..Input::default()
            },
        )
        .await;
    assert_eq!(
        wrong.unwrap_err().to_string(),
        "device code does not match; check the code on your new device"
    );
    let approve = Input {
        id: asked.device_id.clone(),
        // Typed in lower case and without its hyphens.
        code: asked.confirmation.replace('-', "").to_lowercase(),
        ..Input::default()
    };
    let approved = call(&laptop, "approve", approve).await;
    assert_eq!(approved.devices.len(), 2);
    assert!(approved.requests.is_empty());
    let unlocked = call(&desktop, "status", Input::default()).await;
    assert_eq!(unlocked.state, "ready");
    assert_eq!(urls(&unlocked), [link.as_str()]);
    // The approved machine keeps the link itself.
    let imported = fs::read_dir(b.join("shares")).unwrap().count();
    assert_eq!(imported, 1);

    // Revoking: a new epoch, a new vault key and a new incoming-share key.
    let before = held(server).await;
    let before_verified = verify_record(&before, ACCOUNT).unwrap();
    let desktop_box = AgreementKey::from_bytes(base64_32(
        device_file(&b, server).unwrap()["box"].as_str().unwrap(),
    ));
    let old_key = before_verified
        .state
        .unwrap_key(
            &desktop_box,
            &before_verified.state.device(&asked.device_id).unwrap().wrap,
        )
        .unwrap();
    assert_eq!(
        laptop
            .handle(
                "revoke",
                &Input {
                    id: confirmed.device_id.clone(),
                    ..Input::default()
                }
            )
            .await
            .unwrap_err()
            .to_string(),
        "revoke this device from another approved device"
    );
    let revoked = call(
        &laptop,
        "revoke",
        Input {
            id: asked.device_id.clone(),
            ..Input::default()
        },
    )
    .await;
    assert_eq!(revoked.devices.len(), 1);
    let after = verify_record(&held(server).await, ACCOUNT).unwrap();
    assert_eq!(after.state.epoch, before_verified.state.epoch + 1);
    assert!(
        after.open_vault(&old_key).is_err(),
        "the old key opens the new vault"
    );
    assert_ne!(after.state.inbox.as_deref(), Some(inbox.as_str()));
    assert!(after.state.inbox.is_some());
    let gone = call(&desktop, "status", Input::default()).await;
    assert_eq!((gone.state, gone.shares.len()), ("locked", 0));

    // The recovery key, on a machine that never had a device.
    let wrong = spare
        .handle(
            "recover",
            &Input {
                recovery_key: format!("{}AA", &token[..token.len() - 2]),
                ..Input::default()
            },
        )
        .await;
    assert_eq!(wrong.unwrap_err().to_string(), "incorrect recovery key");
    let other = spare
        .handle(
            "recover",
            &Input {
                recovery_key: token.replace("slr1.", "slr1.x"),
                ..Input::default()
            },
        )
        .await;
    assert_eq!(
        other.unwrap_err().to_string(),
        "recovery key does not match this account's recovery identity"
    );
    let recovered = call(
        &spare,
        "recover",
        Input {
            name: "Recovered laptop".to_owned(),
            recovery_key: format!(" {token}\n"),
            ..Input::default()
        },
    )
    .await;
    assert_eq!(recovered.state, "ready");
    assert_eq!(urls(&recovered), [link.as_str()]);
    assert_eq!(recovered.devices.len(), 2);

    // A link the recovered machine publishes reaches the laptop, and not the
    // revoked desktop.
    let (second, second_key) = publish(&c, server, "23456789abcdej", ACCOUNT);
    assert_eq!(spare.backup(ACCOUNT).await, "synced");
    let mut both = vec![link.clone(), second.clone()];
    both.sort();
    assert_eq!(urls(&call(&laptop, "status", Input::default()).await), both);
    assert!(
        call(&desktop, "status", Input::default())
            .await
            .shares
            .is_empty()
    );

    vec![content_key, second_key, token, seed, encode(&old_key)]
}

fn base64_32(text: &str) -> [u8; 32] {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text)
        .unwrap()
        .try_into()
        .unwrap()
}

#[tokio::test]
async fn three_machines_share_one_account() {
    let (server, store) = service().await;
    let secrets = journey(&server, "journey").await;
    let stored = {
        let store = store.lock().unwrap();
        let requests: Vec<Value> = store.requests.iter().map(signed_json).collect();
        format!(
            "{}{requests:?}",
            record_json(store.record.as_ref().unwrap())
        )
    };
    for secret in secrets {
        assert!(!stored.contains(&secret), "{secret} reached the service");
    }
}

/// The journey against the service's own identity store.
#[tokio::test]
async fn three_machines_share_one_account_on_the_services_store() {
    let Ok(bridge) = std::env::var("SLINK_IDENTITY_BRIDGE") else {
        eprintln!("set SLINK_IDENTITY_BRIDGE to the server's scripts/identity-bridge.mjs");
        return;
    };
    let mut child = Command::new("node")
        .arg(bridge)
        .stdout(Stdio::piped())
        .spawn()
        .unwrap();
    let mut line = String::new();
    BufReader::new(child.stdout.take().unwrap())
        .read_line(&mut line)
        .unwrap();
    let started: Value = serde_json::from_str(&line).unwrap();
    let server = started["url"].as_str().unwrap();
    let secrets = journey(server, "bridge").await;
    let mut stored = String::new();
    for entry in walk(Path::new(started["dir"].as_str().unwrap())) {
        stored.push_str(&fs::read_to_string(entry).unwrap_or_default());
    }
    let _ = child.kill();
    let _ = child.wait();
    assert!(!stored.is_empty());
    for secret in secrets {
        assert!(
            !stored.contains(&secret),
            "{secret} reached the service's store"
        );
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

/// The service answers a setup with an identity it made, listing the
/// device's real keys. Neither the answer nor any later one is taken, and
/// nothing is backed up to it.
#[tokio::test]
async fn a_substituted_identity_is_refused_from_setup_on() {
    for lost in [false, true] {
        let (server, store) = service().await;
        store.lock().unwrap().substitute = true;
        store.lock().unwrap().lose_answer = lost;
        let dir = home(&format!("substitute-{lost}"));
        let laptop = Identity::new(&dir, &server, KEY);
        assert!(laptop.handle("setup", &input("Laptop")).await.is_err());
        store.lock().unwrap().lose_answer = false;
        // What was sent is what the device kept before sending it.
        let saved = device_file(&dir, &server).unwrap();
        let sent = verify_record(store.lock().unwrap().record.as_ref().unwrap(), ACCOUNT).unwrap();
        assert_ne!(saved["root"], sent.state.root);
        assert!(saved["pending_setup"].is_object());
        for action in [
            "status",
            "setup",
            "confirm-recovery",
            "sync",
            "discard-setup",
        ] {
            let error = laptop.handle(action, &Input::default()).await.unwrap_err();
            assert!(
                error.to_string().contains("recovery identity"),
                "{action}: {error}"
            );
            assert_ne!(error, Error::SetupConflict, "{action}");
        }
        assert_eq!(device_file(&dir, &server).unwrap(), saved);
        assert_eq!(
            store.lock().unwrap().posts,
            1,
            "a backup reached the substitute"
        );
    }
}

/// A setup whose upload failed, or whose answer was lost, is sent again as
/// the same request, and the recovery key shown is the one that was made.
#[tokio::test]
async fn an_interrupted_setup_is_sent_again_unchanged() {
    for taken in [false, true] {
        let (server, store) = service().await;
        let dir = home(&format!("retry-{taken}"));
        let laptop = Identity::new(&dir, &server, KEY);
        if taken {
            store.lock().unwrap().lose_answer = true;
        } else {
            store.lock().unwrap().offline = true;
        }
        assert!(laptop.handle("setup", &input("Laptop")).await.is_err());
        {
            let mut store = store.lock().unwrap();
            store.offline = false;
            store.lose_answer = false;
            assert_eq!(store.record.is_some(), taken);
        }
        let pending = device_file(&dir, &server).unwrap();
        assert_eq!(pending["revision"], 1);
        assert!(!pending["root"].as_str().unwrap().is_empty());
        let token = pending["pending_recovery"].as_str().unwrap().to_owned();

        let status = call(&laptop, "setup", Input::default()).await;
        assert_eq!(status.state, "ready");
        assert_eq!(status.recovery_key, token);
        let record =
            verify_record(store.lock().unwrap().record.as_ref().unwrap(), ACCOUNT).unwrap();
        assert_eq!(record.state.root, pending["root"]);
        // Sent again unchanged when it was not taken; not sent again when it
        // was.
        let sent = store.lock().unwrap().sent.clone();
        if taken {
            assert_eq!(sent.len(), 1);
        } else {
            assert_eq!(sent.len(), 2);
            assert_eq!(sent[0], sent[1]);
        }
        assert_eq!(sent[0], pending["pending_setup"]);
        let current = device_file(&dir, &server).unwrap();
        assert!(current.get("pending_setup").is_none());
        assert_eq!(current["root"], pending["root"]);
        assert_eq!(current["head"], pending["head"]);

        call(&laptop, "confirm-recovery", Input::default()).await;
        assert!(
            device_file(&dir, &server)
                .unwrap()
                .get("pending_recovery")
                .is_none()
        );
        // An identity this device confirmed is not set up again over.
        store.lock().unwrap().record = None;
        let error = laptop.handle("setup", &Input::default()).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "encrypted identity is missing from the server; refusing to replace it"
        );
    }
}

/// A device whose setup never reached the service, after another set the
/// account up: every action reports the conflict until the setup is
/// discarded, and the device then joins with fresh keys.
#[tokio::test]
async fn a_setup_lost_to_another_device_is_discarded_and_the_device_joins() {
    for join in ["request", "recover"] {
        let (server, store) = service().await;
        let (a, b) = (
            home(&format!("race-{join}-a")),
            home(&format!("race-{join}-b")),
        );
        let (late, first) = (
            Identity::new(&a, &server, KEY),
            Identity::new(&b, &server, KEY),
        );
        store.lock().unwrap().offline = true;
        assert!(late.handle("setup", &input("A")).await.is_err());
        store.lock().unwrap().offline = false;
        let unfinished = device_file(&a, &server).unwrap();

        let made = call(&first, "setup", input("B")).await;
        let posts = store.lock().unwrap().posts;
        for action in [
            "status",
            "setup",
            "request",
            "recover",
            "sync",
            "confirm-recovery",
        ] {
            let error = late
                .handle(
                    action,
                    &Input {
                        recovery_key: made.recovery_key.clone(),
                        ..Input::default()
                    },
                )
                .await
                .unwrap_err();
            assert_eq!(error, Error::SetupConflict, "{action}");
            assert_eq!(
                (error.status(), error.code()),
                (422, Some("setup_conflict"))
            );
        }
        assert_eq!(device_file(&a, &server).unwrap(), unfinished);
        assert_eq!(store.lock().unwrap().posts, posts);

        let discarded = call(&late, "discard-setup", Input::default()).await;
        assert_eq!(discarded.state, "locked");
        assert!(discarded.device_id.is_empty() && !discarded.recovery_pending);
        assert!(device_file(&a, &server).is_none());

        if join == "request" {
            let asked = call(&late, "request", input("A")).await;
            let approve = Input {
                id: asked.device_id,
                code: asked.confirmation,
                ..Input::default()
            };
            call(&first, "approve", approve).await;
        } else {
            let recover = Input {
                name: "A".to_owned(),
                recovery_key: made.recovery_key.clone(),
                ..Input::default()
            };
            call(&late, "recover", recover).await;
        }
        let joined = call(&late, "sync", Input::default()).await;
        assert_eq!((joined.state, joined.devices.len()), ("ready", 2));
        let device = device_file(&a, &server).unwrap();
        assert_ne!(device["seed"], unfinished["seed"]);
        assert_ne!(device["box"], unfinished["box"]);
        assert!(device.get("pending_recovery").is_none());
    }
}

/// Only an unfinished setup is discarded. A device whose setup the service
/// acknowledged keeps refusing an identity that replaced it, and keeps the
/// recovery key it has not shown yet.
#[tokio::test]
async fn only_an_unfinished_setup_is_discarded() {
    let (server, store) = service().await;
    let dir = home("keep");
    let laptop = Identity::new(&dir, &server, KEY);
    call(&laptop, "setup", Input::default()).await;
    let error = laptop
        .handle("discard-setup", &Input::default())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "this device has no unfinished recovery setup to discard"
    );
    let own = store.lock().unwrap().record.take();
    call(
        &Identity::new(&home("keep-other"), &server, KEY),
        "setup",
        Input::default(),
    )
    .await;
    for confirmed in [false, true] {
        let saved = device_file(&dir, &server).unwrap();
        assert_eq!(saved.get("pending_recovery").is_none(), confirmed);
        for action in ["discard-setup", "status"] {
            let error = laptop.handle(action, &Input::default()).await.unwrap_err();
            assert_ne!(error, Error::SetupConflict);
            assert!(error.to_string().contains("recovery identity"), "{error}");
        }
        assert_eq!(device_file(&dir, &server).unwrap(), saved);
        if !confirmed {
            let theirs = store.lock().unwrap().record.replace(own.clone().unwrap());
            call(&laptop, "confirm-recovery", Input::default()).await;
            store.lock().unwrap().record = theirs;
        }
    }
}

/// An answer older than what this device has seen, or one with another
/// root, is refused, and the pins stay where they were.
#[tokio::test]
async fn an_older_or_replaced_identity_is_refused() {
    let (server, store) = service().await;
    let dir = home("pins");
    let laptop = Identity::new(&dir, &server, KEY);
    call(&laptop, "setup", Input::default()).await;
    call(&laptop, "confirm-recovery", Input::default()).await;
    let first = store.lock().unwrap().record.clone().unwrap();
    publish(&dir, &server, "23456789abcdef", ACCOUNT);
    call(&laptop, "sync", Input::default()).await;
    call(&laptop, "sharing", Input::default()).await;
    let pinned = device_file(&dir, &server).unwrap();

    // The first vault again: an older revision.
    store.lock().unwrap().record = Some(first.clone());
    let error = laptop
        .handle("status", &Input::default())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "server returned an older encrypted vault"
    );
    // Another identity, even one that lists this device.
    let own = verify_record(&first, ACCOUNT).unwrap().state.devices[0].clone();
    store.lock().unwrap().record = Some(forged(&own.sign_key, &own.box_key, &own.name));
    let error = laptop
        .handle("status", &Input::default())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "server recovery identity changed; refusing replacement"
    );
    assert_eq!(device_file(&dir, &server).unwrap(), pinned);
    assert_eq!(laptop.backup(ACCOUNT).await, "failed");
}

#[tokio::test]
async fn one_process_at_a_time_holds_the_device_keys() {
    let (server, _store) = service().await;
    let dir = home("lock");
    fs::create_dir_all(dir.join("identity")).unwrap();
    let held = fs::File::create(dir.join("identity").join(".lock")).unwrap();
    held.lock().unwrap();
    let error = Identity::new(&dir, &server, KEY)
        .handle("status", &Input::default())
        .await
        .unwrap_err();
    assert_eq!(
        error.to_string(),
        "another slink process is updating device keys; retry shortly"
    );
    drop(held);
    let status = call(
        &Identity::new(&dir, &server, KEY),
        "status",
        Input::default(),
    )
    .await;
    assert_eq!(status.state, "setup");
}

#[tokio::test]
async fn nothing_is_asked_of_a_server_a_key_may_not_go_to() {
    let dir = home("refused");
    for (server, key, message, status) in [
        (
            "http://example.com",
            KEY,
            "invalid encrypted identity server",
            422,
        ),
        ("https://h/?", KEY, "invalid encrypted identity server", 422),
        ("https://session.link", "", "run slink login first", 401),
    ] {
        let error = Identity::new(&dir, server, key)
            .handle("status", &Input::default())
            .await
            .unwrap_err();
        assert_eq!(
            (error.to_string().as_str(), error.status()),
            (message, status)
        );
    }
    assert!(!dir.join("identity").exists());
    assert_eq!(
        Identity::new(&dir, "https://session.link", KEY)
            .backup("")
            .await,
        "setup"
    );
    assert_eq!(
        Identity::new(&dir, "https://session.link", KEY)
            .backup(ACCOUNT)
            .await,
        "setup"
    );
}

#[tokio::test]
async fn a_status_before_setup_and_an_action_that_needs_one() {
    let (server, store) = service().await;
    let dir = home("before");
    let laptop = Identity::new(&dir, &server, KEY);
    let status = call(&laptop, "status", Input::default()).await;
    assert_eq!(
        status.to_json(),
        json!({"server": server, "account": ACCOUNT, "state": "setup", "devices": [], "requests": [], "shares": [], "recovery_pending": false})
    );
    for action in ["request", "recover", "sync", "approve"] {
        let error = laptop.handle(action, &Input::default()).await.unwrap_err();
        assert_eq!(
            error.to_string(),
            "set up recovery on your first device",
            "{action}"
        );
    }
    assert_eq!(store.lock().unwrap().posts, 0);
    assert!(device_file(&dir, &server).is_none());
    // The service's own refusals keep their status and message.
    call(&laptop, "setup", Input::default()).await;
    let ready = call(&laptop, "status", Input::default()).await.to_json();
    assert_eq!(ready["state"], "ready");
    assert_eq!(ready["recovery_pending"], true);
    assert!(
        ready.get("recovery_key").is_none(),
        "a status shows the key: {ready}"
    );
    let unknown = laptop
        .handle("rotate", &Input::default())
        .await
        .unwrap_err();
    assert_eq!(unknown.to_string(), "unknown device action");
}

/// A backup that loses the race for the vault is made again from the vault
/// the service holds, with both machines' links in it, up to three times in
/// all. Any other write that loses is reported.
#[tokio::test]
async fn a_backup_that_loses_a_race_is_made_again() {
    let (server, store) = service().await;
    let (a, b) = (home("race-sync-a"), home("race-sync-b"));
    let (laptop, desktop) = (
        Identity::new(&a, &server, KEY),
        Identity::new(&b, &server, KEY),
    );
    call(&laptop, "setup", Input::default()).await;
    let asked = call(&desktop, "request", input("Desktop")).await;
    let approve = Input {
        id: asked.device_id,
        code: asked.confirmation,
        ..Input::default()
    };
    call(&laptop, "approve", approve).await;
    let (theirs, _) = publish(&b, &server, "23456789abcdef", ACCOUNT);
    assert_eq!(desktop.backup(ACCOUNT).await, "synced");

    let (mine, _) = publish(&a, &server, "23456789abcdeg", ACCOUNT);
    let posts = store.lock().unwrap().posts;
    store.lock().unwrap().races = 2;
    let synced = call(&laptop, "sync", Input::default()).await;
    let mut both = vec![mine.clone(), theirs.clone()];
    both.sort();
    assert_eq!(urls(&synced), both);
    assert_eq!(store.lock().unwrap().posts, posts + 3);

    publish(&a, &server, "23456789abcdeh", ACCOUNT);
    store.lock().unwrap().races = 3;
    let error = laptop.handle("sync", &Input::default()).await.unwrap_err();
    assert_eq!(
        error.to_string(),
        "vault is changing on another device; retry backup"
    );
    store.lock().unwrap().races = 3;
    assert_eq!(laptop.backup(ACCOUNT).await, "failed");
    // An approval that loses is not made again behind the person's back.
    let asked = call(
        &Identity::new(&home("race-sync-c"), &server, KEY),
        "request",
        input("C"),
    )
    .await;
    store.lock().unwrap().races = 1;
    let approve = Input {
        id: asked.device_id,
        code: asked.confirmation,
        ..Input::default()
    };
    let error = laptop.handle("approve", &approve).await.unwrap_err();
    assert_eq!(
        (error.status(), error.to_string().as_str()),
        (409, "Vault changed; refresh and retry (HTTP 409)")
    );
}
