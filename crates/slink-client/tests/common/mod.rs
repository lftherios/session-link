//! What the tests against a stand-in for the hosted service share: a home of
//! their own, statements and records as the service writes them, its answers
//! and refusals, key bytes that differ on each call, the stand-in on
//! loopback, and the identity store's rule for a write.

#![allow(dead_code)]

use std::fmt::Display;
use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use axum::http::StatusCode;
use axum::response::Response;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use slink_protocol::{Record, Signed, Verified, verify_record};

/// An empty directory of the test's own, for a home.
pub fn home(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "slink-{}-{name}-{}",
        env!("CARGO_CRATE_NAME"),
        std::process::id()
    ));
    let _ = fs::remove_dir_all(&dir);
    fs::create_dir_all(&dir).unwrap();
    dir
}

/// A statement as the service sends one.
pub fn signed(value: &Value) -> Signed {
    Signed {
        payload: value["payload"].as_str().unwrap().to_owned(),
        signature: value["signature"].as_str().unwrap().to_owned(),
    }
}

pub fn signed_json(signed: &Signed) -> Value {
    json!({"payload": signed.payload, "signature": signed.signature})
}

/// An account's record as the service sends it.
pub fn record_json(record: &Record) -> Value {
    json!({
        "events": record.events.iter().map(signed_json).collect::<Vec<_>>(),
        "vault": signed_json(&record.vault),
    })
}

/// The SHA-256 of `bytes` in hex, as the service names a ciphertext.
pub fn sha256(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// An answer with a JSON body.
pub fn reply(status: u16, body: impl Display) -> Response {
    Response::builder()
        .status(StatusCode::from_u16(status).unwrap())
        .header("content-type", "application/json")
        .body(body.to_string().into())
        .unwrap()
}

/// Why the stand-in refused a request: a status and a message.
pub type Refusal = (u16, String);

pub fn refuse<T>(status: u16, message: &str) -> Result<T, Refusal> {
    Err((status, message.to_owned()))
}

static COUNTER: AtomicU64 = AtomicU64::new(1);

/// Bytes for a key, different on each call in a test run.
pub fn random() -> [u8; 32] {
    let mut bytes = [0; 32];
    for (index, byte) in bytes.iter_mut().enumerate() {
        *byte = (index as u8).wrapping_mul(37) ^ (std::process::id() as u8);
    }
    bytes[0] ^= COUNTER.fetch_add(1, Ordering::Relaxed) as u8;
    bytes
}

/// Serves `app` on loopback for as long as the test runs; its address.
pub async fn serve(app: axum::Router) -> String {
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let address = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    format!("http://{address}")
}

/// What a write to an account's record leaves: the record held before it,
/// verified, and the record it makes, as sent and verified.
pub struct Written {
    pub before: Option<Verified>,
    pub record: Record,
    pub verified: Verified,
}

/// A write to `account`'s record as the identity store takes it: only
/// against the head and revision held, and only when the record it makes
/// verifies one revision on.
pub fn write(held: Option<&Record>, account: &str, body: &Value) -> Result<Written, Refusal> {
    let before = held.map(|record| verify_record(record, account).unwrap());
    let (head, revision) = before.as_ref().map_or((String::new(), 0), |held| {
        (held.head.clone(), held.vault.revision)
    });
    if body["expected_head"] != head || body["expected_revision"] != revision {
        return refuse(409, "Vault changed; refresh and retry");
    }
    let mut events = held.map(|record| record.events.clone()).unwrap_or_default();
    events.extend(body.get("event").map(signed));
    let record = Record {
        events,
        vault: signed(&body["vault"]),
    };
    let verified = verify_record(&record, account).map_err(|error| (422, error.to_string()))?;
    if verified.vault.revision != revision + 1 {
        return refuse(409, "Invalid vault revision");
    }
    Ok(Written {
        before,
        record,
        verified,
    })
}
