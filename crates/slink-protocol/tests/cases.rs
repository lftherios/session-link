//! The Rust side of `testdata/protocol`: the envelopes, signed statements,
//! device histories and vault records every implementation is held to
//! (`testdata/protocol/README.md`). The viewer reads the same files in
//! `test/protocol-cases.test.mjs`, and Go in `go/internal/sealed` and
//! `go/internal/identity`.

use std::collections::HashMap;
use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::{Value, json};
use slink_protocol::{
    Error, Kind, Record, Refusal, Signed, State, envelope, verify_history, verify_record,
};

fn read(name: &str) -> Value {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/protocol")
        .join(name);
    let text = fs::read_to_string(&path).unwrap_or_else(|err| panic!("{}: {err}", path.display()));
    serde_json::from_str(&text).expect("the case file is JSON")
}

/// One verdict for every implementation, or one for each.
fn expected(case: &Value) -> &str {
    case["expect"]
        .as_str()
        .or_else(|| case["expect"]["rust"].as_str())
        .expect("the case has a verdict for rust")
}

fn text<'a>(case: &'a Value, field: &str) -> &'a str {
    case[field]
        .as_str()
        .unwrap_or_else(|| panic!("{}: no {field}", case["name"]))
}

fn report(failures: Vec<String>) {
    assert!(failures.is_empty(), "\n{}", failures.join("\n"));
}

#[test]
fn every_envelope_opens_or_is_refused() {
    let cases = read("envelopes.json");
    let cases = cases.as_array().expect("a list of cases");
    let mut opened = 0;
    let mut failures = Vec::new();
    for case in cases {
        let data = STANDARD.decode(text(case, "envelope")).expect("base64");
        let got = envelope::decrypt(&data, text(case, "key"));
        match (expected(case), got) {
            ("refused", Err(_)) => {}
            ("opens", Ok(plain)) if STANDARD.encode(&plain) == text(case, "plaintext") => {
                opened += 1;
            }
            (want, got) => failures.push(format!(
                "{}: want {want}, got {:?}",
                case["name"],
                got.map(|plain| plain.len())
            )),
        }
    }
    report(failures);
    assert!(opened > 0 && opened < cases.len(), "{opened} cases open");
}

#[test]
fn every_statement_verifies_or_is_refused() {
    let cases = read("statements.json");
    let cases = cases.as_array().expect("a list of cases");
    let mut verified = 0;
    let mut failures = Vec::new();
    for case in cases {
        let kind = Kind::ALL
            .into_iter()
            .find(|kind| kind.as_str() == text(case, "kind"))
            .expect("a kind this crate knows");
        let signed = Signed {
            payload: text(case, "payload").to_owned(),
            signature: text(case, "signature").to_owned(),
        };
        let got = signed.verify(kind, text(case, "public"));
        match (expected(case), got) {
            ("verified", Ok(())) => verified += 1,
            ("refused", Err(_)) => {}
            (want, got) => failures.push(format!("{}: want {want}, got {got:?}", case["name"])),
        }
    }
    report(failures);
    assert!(verified > 0 && verified < cases.len(), "{verified} verify");
}

struct Histories {
    signed: HashMap<String, Signed>,
    cases: Vec<Value>,
}

fn histories() -> Histories {
    let file = read("histories.json");
    let signed = file["signed"]
        .as_object()
        .expect("the statements by id")
        .iter()
        .map(|(id, value)| {
            let signed = Signed {
                payload: text(value, "payload").to_owned(),
                signature: text(value, "signature").to_owned(),
            };
            (id.clone(), signed)
        })
        .collect();
    let cases = file["cases"].as_array().expect("a list of cases").clone();
    Histories { signed, cases }
}

impl Histories {
    fn events(&self, case: &Value) -> Vec<Signed> {
        let ids = case["events"].as_array().expect("a list of events");
        ids.iter()
            .map(|id| self.signed[id.as_str().expect("an id")].clone())
            .collect()
    }

    /// What this crate makes of a case: the state it reads, or the rule the
    /// history or the vault broke.
    fn judge(&self, case: &Value) -> Result<Value, &'static str> {
        let events = self.events(case);
        let account = text(case, "account");
        let (state, head, revision) = match case["vault"].as_str() {
            None => {
                let history = verify_history(&events, account).map_err(rule)?;
                (history.state, history.head, None)
            }
            Some(id) => {
                let record = Record {
                    events,
                    vault: self.signed[id].clone(),
                };
                let verified = verify_record(&record, account).map_err(rule)?;
                (verified.state, verified.head, Some(verified.vault.revision))
            }
        };
        let mut summary = summary(&state, &head);
        if let Some(revision) = revision {
            summary["revision"] = json!(revision);
        }
        Ok(summary)
    }
}

/// What a verifier must report of a history it accepts.
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

/// The name a case uses for the rule behind an error.
fn rule(error: Error) -> &'static str {
    match error {
        Error::History(rule) | Error::Vault(rule) => rule.as_str(),
        Error::Signature(_) => "signature",
        Error::SigningKey => "key",
        Error::Encoding | Error::TooLarge => "signed",
        Error::Envelope | Error::ContentKey | Error::Decrypt => "envelope",
    }
}

#[test]
fn every_history_and_record_gets_its_verdict() {
    let file = histories();
    let (mut valid, mut invalid) = (0, 0);
    let mut failures = Vec::new();
    for case in &file.cases {
        let name = &case["name"];
        match (expected(case), file.judge(case)) {
            ("valid", Ok(state)) => {
                valid += 1;
                if state != case["state"] {
                    failures.push(format!("{name}: read {state}"));
                }
            }
            // A refusal counts only if it is for the rule the case is about.
            ("invalid", Err(rule)) => {
                invalid += 1;
                if case["rule"] != rule {
                    failures.push(format!("{name}: refused for {rule}, not {}", case["rule"]));
                }
            }
            (want, Ok(_)) => failures.push(format!("{name}: want {want}, accepted")),
            (want, Err(rule)) => failures.push(format!("{name}: want {want}, refused for {rule}")),
        }
    }
    report(failures);
    assert!(
        valid > 20 && invalid > 100,
        "{valid} valid, {invalid} invalid"
    );
}

/// A rule with no case could be dropped from every implementation unnoticed.
#[test]
fn every_rule_has_a_case() {
    let file = histories();
    for rule in Refusal::ALL {
        assert!(
            file.cases.iter().any(|case| case["rule"] == rule.as_str()),
            "no case is about {}",
            rule.as_str()
        );
    }
    for case in &file.cases {
        if let Some(name) = case["rule"].as_str() {
            assert!(
                Refusal::ALL.iter().any(|rule| rule.as_str() == name),
                "{}: {name} is not a rule",
                case["name"]
            );
        }
    }
}

/// Where Rust's verdict differs from another implementation's, it is Rust
/// that refuses: nothing this crate accepts is refused by Go or the viewer.
#[test]
fn no_recorded_difference_has_rust_accepting() {
    let file = histories();
    let mut differences = 0;
    for case in &file.cases {
        let Some(each) = case["expect"].as_object() else {
            continue;
        };
        differences += 1;
        assert_eq!(each["rust"], "invalid", "{}", case["name"]);
        assert!(case["why"].is_string(), "{}: no reason", case["name"]);
    }
    assert!(differences > 20, "{differences} recorded differences");
}

/// A signed text has one spelling: change any one character of the last
/// event's payload or signature, or of the vault's, and the record is refused.
#[test]
fn no_single_character_of_a_signed_text_can_change() {
    let file = histories();
    let case = file
        .cases
        .iter()
        .find(|case| case["name"] == "a vault after a revocation")
        .expect("the case");
    let account = text(case, "account");
    let record = Record {
        events: file.events(case),
        vault: file.signed[text(case, "vault")].clone(),
    };
    verify_record(&record, account).expect("the record as written");

    // Any other character of the alphabet, and one outside it.
    let other = |character: u8| match character {
        b'A' => b'B',
        b'=' => b'A',
        _ => b'A',
    };
    let mut tried = 0;
    let last = record.events.len() - 1;
    for target in 0..4 {
        let original = match target {
            0 => &record.events[last].payload,
            1 => &record.events[last].signature,
            2 => &record.vault.payload,
            _ => &record.vault.signature,
        };
        for at in 0..original.len() {
            for replacement in [other(original.as_bytes()[at]), b'='] {
                let mut bytes = original.clone().into_bytes();
                bytes[at] = replacement;
                let changed = String::from_utf8(bytes).expect("ASCII");
                let mut record = record.clone();
                match target {
                    0 => record.events[last].payload = changed,
                    1 => record.events[last].signature = changed,
                    2 => record.vault.payload = changed,
                    _ => record.vault.signature = changed,
                }
                assert!(
                    verify_record(&record, account).is_err(),
                    "accepted with character {at} of text {target} changed"
                );
                tried += 1;
            }
        }
    }
    assert!(tried > 3000, "{tried} changes tried");
}
