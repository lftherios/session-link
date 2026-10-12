//! Malformed input fails safely. The cases in `testdata/protocol` are wrong
//! in ways someone thought of; these are wrong at random, and correctly
//! signed, so that they reach the code behind the signature check. Whatever
//! arrives, the answer is a verified value or one error, never a panic, and
//! the run is the same every time.
//!
//! The same generator runs on every target the binary is built for.

use std::fs;
use std::path::Path;

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use ring::digest::{SHA256, digest};
use ring::signature::{Ed25519KeyPair, KeyPair};
use serde_json::Value;
use slink_protocol::{
    Error, Kind, MAX_DEVICE_NAME, Record, Refusal, Signed, envelope, verify_history, verify_record,
};

const ACCOUNT: &str = "usr_fixture";

/// xorshift64: enough to choose where to damage something.
struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }

    fn below(&mut self, limit: usize) -> usize {
        (self.next() % limit.max(1) as u64) as usize
    }

    fn bytes(&mut self, length: usize) -> Vec<u8> {
        (0..length).map(|_| self.next() as u8).collect()
    }
}

/// A fixture key: `scripts/protocol-cases.mjs` derives every seed from a
/// label in the same way.
fn key(label: &str) -> Ed25519KeyPair {
    let seed = digest(
        &SHA256,
        format!("session.link fixture/{label}/0").as_bytes(),
    );
    Ed25519KeyPair::from_seed_unchecked(seed.as_ref()).expect("a seed")
}

fn sign(kind: Kind, raw: &[u8], key: &Ed25519KeyPair) -> Signed {
    let message = [b"slink/", kind.as_str().as_bytes(), b"/v1\x00", raw].concat();
    Signed {
        payload: URL_SAFE_NO_PAD.encode(raw),
        signature: URL_SAFE_NO_PAD.encode(key.sign(&message)),
    }
}

/// The events of a named case in `testdata/protocol/histories.json`.
fn events(name: &str) -> Vec<Signed> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/protocol/histories.json");
    let file: Value = serde_json::from_str(&fs::read_to_string(path).expect("the cases")).unwrap();
    let cases = file["cases"].as_array().unwrap();
    let case = cases.iter().find(|case| case["name"] == name).expect(name);
    let ids = case["events"].as_array().unwrap();
    ids.iter()
        .map(|id| {
            let signed = &file["signed"][id.as_str().unwrap()];
            Signed {
                payload: signed["payload"].as_str().unwrap().to_owned(),
                signature: signed["signature"].as_str().unwrap().to_owned(),
            }
        })
        .collect()
}

/// One to three small injuries: a bit, a byte, a piece removed, added, copied
/// or cut off, or a character JSON gives meaning to.
fn damage(random: &mut Random, original: &[u8]) -> Vec<u8> {
    const MEANINGFUL: &[u8] = b"{}[]\",:\\0129-.eEntf \n\x00\xff";
    let mut bytes = original.to_vec();
    for _ in 0..=random.below(3) {
        if bytes.is_empty() {
            bytes = random.bytes(4);
        }
        let at = random.below(bytes.len());
        match random.below(8) {
            0 => bytes[at] ^= 1 << random.below(8),
            1 => bytes[at] = random.next() as u8,
            2 => {
                let end = (at + 1 + random.below(8)).min(bytes.len());
                bytes.drain(at..end);
            }
            3 => {
                let extra = random.below(8) + 1;
                let extra = random.bytes(extra);
                bytes.splice(at..at, extra);
            }
            4 => {
                let from = random.below(bytes.len());
                let end = (from + 1 + random.below(24)).min(bytes.len());
                let piece = bytes[from..end].to_vec();
                bytes.splice(at..at, piece);
            }
            5 => bytes.truncate(at),
            6 => bytes[at] = MEANINGFUL[random.below(MEANINGFUL.len())],
            _ => {
                let other = random.below(bytes.len());
                bytes.swap(at, other);
            }
        }
    }
    bytes
}

#[test]
fn the_fixture_keys_sign_what_the_cases_hold() {
    // Ed25519 signatures are deterministic, so ring with the label's seed
    // must write the very signature Node wrote into the case file.
    let first = &events("the first event alone")[0];
    let raw = first.read(Kind::Event).unwrap();
    assert_eq!(sign(Kind::Event, &raw, &key("usr_fixture/root")), *first);
    let state: Value = serde_json::from_slice(&raw).unwrap();
    let root = URL_SAFE_NO_PAD.encode(key("usr_fixture/root").public_key());
    assert_eq!(state["root"], root.as_str());
}

#[test]
fn a_damaged_first_event_is_refused_or_still_a_first_event() {
    let original = events("the first event alone")[0]
        .read(Kind::Event)
        .unwrap();
    let root = key("usr_fixture/root");
    let mut random = Random(0x5eed_0001);
    let (mut accepted, mut refused) = (0, 0);
    for _ in 0..4000 {
        let raw = damage(&mut random, &original);
        match verify_history(&[sign(Kind::Event, &raw, &root)], ACCOUNT) {
            Ok(history) => {
                accepted += 1;
                let state = history.state;
                assert_eq!(state.account, ACCOUNT);
                assert_eq!((state.seq, state.epoch, state.devices.len()), (0, 1, 1));
                assert_eq!(state.signer, "root");
                assert!(state.devices[0].name.len() <= MAX_DEVICE_NAME);
            }
            Err(Error::History(_)) => refused += 1,
            Err(other) => panic!("a history failed as {other:?}"),
        }
    }
    // Damage to a name or to the inside of a wrapper leaves a first event
    // that is still one; nearly everything else does not.
    assert!(refused > 3000 && accepted > 0, "{accepted} accepted");
}

#[test]
fn a_damaged_later_event_is_refused_or_still_follows() {
    let chain = events("a device approves a second");
    let original = chain[1].read(Kind::Event).unwrap();
    let laptop = key("usr_fixture/laptop/sign");
    let before = verify_history(&chain[..1], ACCOUNT).unwrap();
    let mut random = Random(0x5eed_0002);
    let (mut accepted, mut refused) = (0, 0);
    for _ in 0..4000 {
        let raw = damage(&mut random, &original);
        let events = [chain[0].clone(), sign(Kind::Event, &raw, &laptop)];
        match verify_history(&events, ACCOUNT) {
            Ok(history) => {
                accepted += 1;
                let state = history.state;
                assert_eq!((state.seq, state.epoch), (1, 1));
                assert_eq!(state.prev, before.head);
                assert_eq!(state.root, before.state.root);
                // The first device is still there, as it was.
                assert_eq!(
                    state.device(&before.state.devices[0].id),
                    before.state.devices.first()
                );
            }
            Err(Error::History(_)) => refused += 1,
            Err(other) => panic!("a history failed as {other:?}"),
        }
    }
    assert!(refused > 3000, "{accepted} accepted, {refused} refused");
}

#[test]
fn a_damaged_vault_is_refused_or_still_this_accounts() {
    let chain = events("the first event alone");
    let head = verify_history(&chain, ACCOUNT).unwrap().head;
    let laptop = key("usr_fixture/laptop/sign");
    let state: Value = serde_json::from_slice(&chain[0].read(Kind::Event).unwrap()).unwrap();
    let original = serde_json::json!({
        "account": ACCOUNT,
        "revision": 3,
        "head": head,
        "epoch": 1,
        "signer": state["devices"][0]["id"],
        "data": URL_SAFE_NO_PAD.encode([7; 40]),
    })
    .to_string();
    let record = |vault| Record {
        events: chain.clone(),
        vault,
    };
    verify_record(
        &record(sign(Kind::Vault, original.as_bytes(), &laptop)),
        ACCOUNT,
    )
    .unwrap();
    let mut random = Random(0x5eed_0003);
    let mut refused = 0;
    for _ in 0..4000 {
        let raw = damage(&mut random, original.as_bytes());
        match verify_record(&record(sign(Kind::Vault, &raw, &laptop)), ACCOUNT) {
            Ok(verified) => {
                assert_eq!(verified.vault.account, ACCOUNT);
                assert_eq!(verified.vault.head, head);
                assert!(verified.vault.revision >= 1);
            }
            Err(Error::Vault(_)) => refused += 1,
            Err(other) => panic!("a vault failed as {other:?}"),
        }
    }
    assert!(refused > 3000, "{refused} refused");
}

#[test]
fn text_that_was_never_signed_is_refused() {
    let mut random = Random(0x5eed_0004);
    let chain = events("the first event alone");
    for round in 0..1500 {
        // Random bytes as they are, and random bytes spelt as base64url.
        let length = random.below(200);
        let noise = random.bytes(length);
        let text = String::from_utf8_lossy(&noise).into_owned();
        let spelt = URL_SAFE_NO_PAD.encode(&noise);
        for payload in [text.clone(), spelt.clone()] {
            for signature in [text.clone(), spelt.clone(), chain[0].signature.clone()] {
                let signed = Signed {
                    payload: payload.clone(),
                    signature,
                };
                assert!(verify_history(std::slice::from_ref(&signed), ACCOUNT).is_err());
                let kind = Kind::ALL[round % Kind::ALL.len()];
                assert!(signed.verify(kind, &spelt).is_err());
                assert!(signed.verify(kind, &text).is_err());
                let record = Record {
                    events: chain.clone(),
                    vault: signed,
                };
                assert!(verify_record(&record, ACCOUNT).is_err());
            }
        }
    }
}

#[test]
fn depth_and_size_are_refused_before_they_cost_anything() {
    let chain = events("the first event alone");
    let root = key("usr_fixture/root");
    let laptop = key("usr_fixture/laptop/sign");
    let nested = |depth: usize| format!("{}{}", "[".repeat(depth), "]".repeat(depth)).into_bytes();
    // As deep as an event's size allows, and correctly signed.
    let deep = sign(Kind::Event, &nested(12_000), &root);
    assert_eq!(
        verify_history(&[deep], ACCOUNT),
        Err(Error::History(Refusal::Payload))
    );
    // An event past the size limit is not decoded at all.
    let long = sign(Kind::Event, &nested(13_000), &root);
    assert_eq!(
        verify_history(&[long], ACCOUNT),
        Err(Error::History(Refusal::Signed))
    );
    // A vault may be megabytes, and a million levels of it are still only
    // text until the parser gives up at its limit.
    let record = Record {
        events: chain.clone(),
        vault: sign(Kind::Vault, &nested(1_000_000), &laptop),
    };
    assert_eq!(
        verify_record(&record, ACCOUNT),
        Err(Error::Vault(Refusal::Payload))
    );
    let record = Record {
        events: chain,
        vault: sign(Kind::Vault, &vec![b' '; 7 * 1024 * 1024], &laptop),
    };
    assert_eq!(verify_record(&record, ACCOUNT), Err(Error::TooLarge));
    // More events than a history may hold, whatever they are.
    let many = vec![
        Signed {
            payload: String::new(),
            signature: String::new(),
        };
        513
    ];
    assert_eq!(
        verify_history(&many, ACCOUNT),
        Err(Error::History(Refusal::Length))
    );
    assert_eq!(
        verify_history(&[], ACCOUNT),
        Err(Error::History(Refusal::Length))
    );
}

#[test]
fn a_damaged_envelope_never_opens() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../testdata/encryption/go.json");
    let fixture: Value = serde_json::from_str(&fs::read_to_string(path).unwrap()).unwrap();
    let key = fixture["key"].as_str().unwrap();
    let original = base64::engine::general_purpose::STANDARD
        .decode(fixture["envelope"].as_str().unwrap())
        .unwrap();
    assert_eq!(
        envelope::decrypt(&original, key).unwrap(),
        fixture["plaintext"].as_str().unwrap().as_bytes()
    );
    let mut random = Random(0x5eed_0005);
    for _ in 0..10_000 {
        let data = damage(&mut random, &original);
        if data != original {
            assert!(envelope::decrypt(&data, key).is_err());
        }
        let noise_length = random.below(64);
        let noise = random.bytes(noise_length);
        assert!(envelope::decrypt(&noise, key).is_err());
        let text = String::from_utf8_lossy(&noise);
        assert!(envelope::decrypt(&original, &text).is_err());
    }
}
