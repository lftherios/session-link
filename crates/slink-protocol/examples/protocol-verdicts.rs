//! The Rust side of `scripts/protocol-diff.mjs`. Each line on stdin is one
//! case as JSON: an envelope and a key, a signed statement and a public key,
//! a box, a key wrapper, a device request, or a history with or without its
//! vault. Each line on stdout is what this crate makes of it.

use std::io::{self, BufRead, Write};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use slink_protocol::encoding::decode_exact;
use slink_protocol::{
    AgreementKey, Error, Kind, Record, Request, Signed, State, confirmation, envelope, unseal,
    unwrap, verify_history, verify_record,
};

fn main() -> io::Result<()> {
    let mut out = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let case: Value = serde_json::from_str(&line?)?;
        writeln!(out, "{}", judge(&case))?;
    }
    out.flush()
}

fn judge(case: &Value) -> Value {
    let text = |field: &str| case[field].as_str().unwrap_or_default();
    match text("t") {
        "envelope" => {
            let data = STANDARD.decode(text("envelope")).unwrap_or_default();
            match envelope::decrypt(&data, text("key")) {
                Ok(plain) => json!({ "verdict": "opens", "plaintext": STANDARD.encode(plain) }),
                Err(error) => json!({ "verdict": "refused", "error": error.to_string() }),
            }
        }
        "statement" => {
            let verified = Kind::ALL
                .into_iter()
                .find(|kind| kind.as_str() == text("kind"))
                .ok_or(Error::Encoding)
                .and_then(|kind| signed(case).verify(kind, text("public")));
            match verified {
                Ok(()) => json!({ "verdict": "verified" }),
                Err(error) => json!({ "verdict": "refused", "error": error.to_string() }),
            }
        }
        "box" => {
            let opened = decode_exact(text("key"))
                .and_then(|key| unseal(&key, text("box"), text("context")));
            match opened {
                Ok(plain) => json!({ "verdict": "opens", "plaintext": STANDARD.encode(plain) }),
                Err(error) => json!({ "verdict": "refused", "error": error.to_string() }),
            }
        }
        "wrapper" => {
            let opened = decode_exact(text("private")).and_then(|private| {
                let private = AgreementKey::from_bytes(private);
                unwrap(&private, text("wrapper"), text("context"))
            });
            match opened {
                Ok(key) => json!({ "verdict": "opens", "plaintext": STANDARD.encode(key) }),
                Err(error) => json!({ "verdict": "refused", "error": error.to_string() }),
            }
        }
        "request" => {
            let statement = signed(case);
            let verified = Request::verify(&statement)
                .and_then(|request| Ok((confirmation(&statement)?, request)));
            match verified {
                Ok((code, request)) => json!({ "verdict": "verified", "state": {
                    "id": request.id(),
                    "account": request.account,
                    "root": request.root,
                    "name": request.name,
                    "created": request.created,
                    "code": code,
                } }),
                Err(error) => {
                    json!({ "verdict": "refused", "rule": rule(error), "error": error.to_string() })
                }
            }
        }
        _ => {
            let events: Vec<Signed> = case["events"]
                .as_array()
                .map(|events| events.iter().map(signed).collect())
                .unwrap_or_default();
            let account = text("account");
            let verified = if case["vault"].is_object() {
                let record = Record {
                    events,
                    vault: signed(&case["vault"]),
                };
                verify_record(&record, account).map(|verified| {
                    let mut state = summary(&verified.state, &verified.head);
                    state["revision"] = json!(verified.vault.revision);
                    state
                })
            } else {
                verify_history(&events, account)
                    .map(|history| summary(&history.state, &history.head))
            };
            match verified {
                Ok(state) => json!({ "verdict": "valid", "state": state }),
                Err(error) => {
                    json!({ "verdict": "invalid", "rule": rule(error), "error": error.to_string() })
                }
            }
        }
    }
}

fn signed(value: &Value) -> Signed {
    Signed {
        payload: value["payload"].as_str().unwrap_or_default().to_owned(),
        signature: value["signature"].as_str().unwrap_or_default().to_owned(),
    }
}

fn summary(state: &State, head: &str) -> Value {
    let devices: Vec<Value> = state
        .devices
        .iter()
        .map(|device| json!({ "id": device.id, "name": device.name }))
        .collect();
    json!({
        "head": head,
        "seq": state.seq,
        "kind": state.kind.as_str(),
        "signer": state.signer,
        "epoch": state.epoch,
        "inbox": state.inbox.as_deref().unwrap_or(""),
        "devices": devices,
    })
}

fn rule(error: Error) -> &'static str {
    match error {
        Error::History(rule) | Error::Vault(rule) => rule.as_str(),
        Error::Signature(_) => "signature",
        Error::SigningKey => "key",
        Error::Encoding | Error::TooLarge => "signed",
        Error::Envelope | Error::ContentKey | Error::Decrypt => "envelope",
        Error::Request => "request",
        // What a box, a wrapper or a recovery key is refused with; no
        // history, record or request is.
        _ => "keys",
    }
}
