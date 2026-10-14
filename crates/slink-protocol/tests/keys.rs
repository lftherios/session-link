//! The making side, held to `testdata/protocol/keys.json`: boxes, wrappers,
//! signatures, device requests, an account opened by a device and by its
//! recovery key, and shares for named people
//! (`testdata/protocol/README.md`).
//!
//! The cases were made with `node:crypto` from keys and nonces the file
//! gives. Given the same ones this crate must write the same bytes, and it
//! must open what is there, so whatever it makes the Go CLI and the viewer
//! open too: they read the same file in `go/internal/identity` and
//! `test/protocol-cases.test.mjs`.

mod common;

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use serde_json::Value;
use slink_protocol::encoding::{decode, decode_exact, encode};
use slink_protocol::{
    Accepted, AgreementKey, Error, Invitation, Invite, Kind, Offer, Record, Recovery, Request,
    Signed, SigningKey, Verified, binding_proof, code_matches, commitment, confirmation, envelope,
    named_context, open_recovery, recovery_token, recovery_token_matches, seal, unseal, unwrap,
    verify_claim, verify_history, verify_record, wrap,
};

use common::{expected, read, report, signed, text};

fn section(name: &str) -> Vec<Value> {
    let file = read("keys.json");
    let cases = file[name].as_array().unwrap_or_else(|| panic!("no {name}"));
    assert!(!cases.is_empty(), "no cases in {name}");
    cases.clone()
}

fn key(case: &Value, field: &str) -> [u8; 32] {
    decode_exact(text(case, field)).unwrap_or_else(|_| panic!("{}: {field}", case["name"]))
}

fn bytes(case: &Value, field: &str) -> Vec<u8> {
    STANDARD.decode(text(case, field)).expect("base64")
}

/// The nonce a box was sealed with: its first twelve bytes.
fn nonce_of(sealed: &str) -> [u8; 12] {
    *decode(sealed)
        .expect("a box")
        .first_chunk()
        .expect("a nonce")
}

/// Runs every case of a section. `judge` returns what was opened, and checks
/// for itself whatever a case that opens says about it.
fn run<T>(
    name: &str,
    opens: &str,
    judge: impl Fn(&Value) -> Result<T, String>,
    check: impl Fn(&Value, T) -> Result<(), String>,
) {
    let cases = section(name);
    let mut opened = 0;
    let mut failures = Vec::new();
    for case in &cases {
        let label = &case["name"];
        match (expected(case), judge(case)) {
            ("refused", Err(_)) => {}
            (want, Ok(value)) if want == opens => {
                opened += 1;
                if let Err(problem) = check(case, value) {
                    failures.push(format!("{label}: {problem}"));
                }
            }
            (want, Ok(_)) => failures.push(format!("{label}: want {want}, got {opens}")),
            (want, Err(error)) => failures.push(format!("{label}: want {want}, refused: {error}")),
        }
    }
    report(failures);
    assert!(
        opened > 0 && opened < cases.len(),
        "{name}: {opened} of {} cases are accepted; both kinds are needed",
        cases.len()
    );
}

fn same<T: PartialEq + std::fmt::Debug>(what: &str, got: T, want: T) -> Result<(), String> {
    if got == want {
        Ok(())
    } else {
        Err(format!("{what} is {got:?}, want {want:?}"))
    }
}

#[test]
fn every_box_opens_or_is_refused_and_is_made_again() {
    run(
        "boxes",
        "opens",
        |case| {
            unseal(&key(case, "key"), text(case, "box"), text(case, "context"))
                .map_err(|error| error.to_string())
        },
        |case, plain| {
            same("the plaintext", &plain, &bytes(case, "plaintext"))?;
            let sealed = text(case, "box");
            let made = seal(
                &key(case, "key"),
                &plain,
                text(case, "context"),
                nonce_of(sealed),
            );
            same("the box made again", made.as_str(), sealed)
        },
    );
}

#[test]
fn every_wrapper_opens_or_is_refused_and_is_made_again() {
    let made = std::cell::Cell::new(0);
    run(
        "wrappers",
        "opens",
        |case| {
            let private = AgreementKey::from_bytes(key(case, "private"));
            unwrap(&private, text(case, "wrapper"), text(case, "context"))
                .map_err(|error| error.to_string())
        },
        |case, wrapped| {
            same("the key", wrapped, key(case, "key"))?;
            // Where the case says how it was made: the one-time key.
            let Some(ephemeral) = case["ephemeral"].as_str() else {
                return Ok(());
            };
            made.set(made.get() + 1);
            let private = AgreementKey::from_bytes(key(case, "private"));
            same(
                "the public key",
                private.public().as_str(),
                text(case, "public"),
            )?;
            let wrapper = text(case, "wrapper");
            let (_, sealed) = wrapper.split_once('.').expect("a wrapper");
            let again = wrap(
                text(case, "public"),
                &wrapped,
                text(case, "context"),
                &AgreementKey::from_bytes(decode_exact(ephemeral).expect("a key")),
                nonce_of(sealed),
            )
            .map_err(|error| error.to_string())?;
            same("the wrapper made again", again.as_str(), wrapper)
        },
    );
    assert!(made.get() >= 5, "{} wrappers were made again", made.get());
}

#[test]
fn nothing_is_wrapped_to_a_key_of_small_order() {
    run(
        "recipients",
        "wraps",
        |case| {
            wrap(
                text(case, "public"),
                &[7; 32],
                "context",
                &AgreementKey::from_bytes([8; 32]),
                [9; 12],
            )
            .map_err(|error| error.to_string())
        },
        |_, wrapper| same("the wrapper's length", wrapper.len(), 124),
    );
}

fn kind(case: &Value) -> Kind {
    Kind::ALL
        .into_iter()
        .find(|kind| kind.as_str() == text(case, "kind"))
        .expect("a kind this crate knows")
}

#[test]
fn a_seed_and_a_payload_have_one_signature() {
    let cases = section("signatures");
    let mut kinds = Vec::new();
    for case in &cases {
        let name = &case["name"];
        let key = SigningKey::from_seed(key(case, "seed"));
        assert_eq!(key.public(), text(case, "public"), "{name}");
        let want = signed(case);
        let raw = decode(&want.payload).expect("a payload");
        let made = Signed::sign(kind(case), &raw, &key).expect("within the limit");
        assert_eq!(made, want, "{name}");
        made.verify(kind(case), &key.public())
            .unwrap_or_else(|error| panic!("{name}: {error}"));
        kinds.push(kind(case));
    }
    for kind in Kind::ALL {
        assert!(kinds.contains(&kind), "no case signs as {}", kind.as_str());
    }
}

#[test]
fn every_request_is_verified_or_refused_and_is_made_again() {
    let made = std::cell::Cell::new(0);
    run(
        "requests",
        "verified",
        |case| Request::verify(&signed(case)).map_err(|error| error.to_string()),
        |case, request| {
            let statement = signed(case);
            same("the id", request.id().as_str(), text(case, "id"))?;
            let code = confirmation(&statement).map_err(|error| error.to_string())?;
            same("the code", code.as_str(), text(case, "code"))?;
            if !code_matches(&code.to_lowercase().replace('-', " "), &statement) {
                return Err("the code typed in small letters does not match".into());
            }
            // Where the case is what a client writes: the device's seed.
            let Some(seed) = case["seed"].as_str() else {
                return Ok(());
            };
            made.set(made.get() + 1);
            let key = SigningKey::from_seed(decode_exact(seed).expect("a seed"));
            same("the signing key", key.public().as_str(), &request.sign_key)?;
            let again = Signed::sign(Kind::Request, &request.encode(), &key)
                .map_err(|error| error.to_string())?;
            same("the request made again", again, statement)
        },
    );
    assert!(made.get() >= 5, "{} requests were made again", made.get());
}

/// Where Rust's verdict differs from another implementation's, it is Rust
/// that refuses: nothing this crate opens or verifies is refused elsewhere.
#[test]
fn no_recorded_difference_has_rust_accepting() {
    let file = read("keys.json");
    let mut differences = 0;
    for (section, cases) in file.as_object().expect("the sections") {
        for case in cases.as_array().expect("a list of cases") {
            let Some(each) = case["expect"].as_object() else {
                continue;
            };
            differences += 1;
            assert_eq!(each["rust"], "refused", "{section}: {}", case["name"]);
            assert!(
                case["why"].is_string(),
                "{section}: {}: no reason",
                case["name"]
            );
        }
    }
    assert!(differences >= 10, "{differences} recorded differences");
}

/// The record of a case, which always verifies: what a case is about is
/// whether it can then be opened.
fn record(case: &Value) -> Verified {
    let events = case["events"].as_array().expect("a list of events");
    let record = Record {
        events: events.iter().map(signed).collect(),
        vault: signed(&case["vault"]),
    };
    verify_record(&record, text(case, "account"))
        .unwrap_or_else(|error| panic!("{}: the record does not verify: {error}", case["name"]))
}

/// What a case that opens says of the vault: its key, what it holds, and
/// that this crate seals the same bytes again and writes the same statement.
fn check_vault(case: &Value, verified: &Verified, vault_key: [u8; 32]) -> Result<(), String> {
    same("the vault key", vault_key, key(case, "vault_key"))?;
    let contents = verified
        .open_vault(&vault_key)
        .map_err(|error| error.to_string())?;
    same("the contents", &contents, &bytes(case, "contents"))?;
    let data = &verified.vault.data;
    let sealed = verified
        .state
        .seal_vault(&vault_key, &contents, nonce_of(data));
    same("the vault sealed again", &sealed, data)?;
    let written = encode(&verified.vault.encode());
    same(
        "the vault statement written again",
        written.as_str(),
        text(&case["vault"], "payload"),
    )
}

#[test]
fn a_device_opens_its_account_or_is_refused() {
    run(
        "devices",
        "opens",
        |case| {
            let verified = record(case);
            let device = verified
                .state
                .device(text(case, "device"))
                .ok_or("the history does not hold the device")?
                .clone();
            let private = AgreementKey::from_bytes(key(case, "private"));
            let vault_key = verified
                .state
                .unwrap_key(&private, &device.wrap)
                .map_err(|error| error.to_string())?;
            verified
                .open_vault(&vault_key)
                .map_err(|error| error.to_string())?;
            Ok((verified, vault_key))
        },
        |case, (verified, vault_key)| check_vault(case, &verified, vault_key),
    );
}

#[test]
fn a_recovery_key_opens_its_account_or_is_refused() {
    run(
        "recoveries",
        "opens",
        |case| {
            let verified = record(case);
            let recovery =
                open_recovery(text(case, "token"), &verified.state).map_err(|e| e.to_string())?;
            let vault_key = verified
                .state
                .unwrap_key(&recovery.agreement, &verified.state.recovery_wrap)
                .map_err(|error| error.to_string())?;
            verified
                .open_vault(&vault_key)
                .map_err(|error| error.to_string())?;
            Ok((verified, recovery, vault_key))
        },
        |case, (verified, recovery, vault_key)| {
            let state = &verified.state;
            same("the root seed", *recovery.root.seed(), key(case, "root"))?;
            same(
                "the agreement key",
                *recovery.agreement.bytes(),
                key(case, "agreement"),
            )?;
            check_vault(case, &verified, vault_key)?;
            // The key a person holds and the package it opens, made again.
            let token = text(case, "token").trim();
            let secret: [u8; 32] =
                decode_exact(token.rsplit('.').next().expect("a secret")).expect("32 bytes");
            same(
                "the recovery key made again",
                recovery_token(&state.account, &state.root, &secret).as_str(),
                token,
            )?;
            let again = Recovery {
                root: SigningKey::from_seed(key(case, "root")),
                agreement: AgreementKey::from_bytes(key(case, "agreement")),
            }
            .seal(&state.account, &secret, nonce_of(&state.recovery_box));
            same("the package made again", &again, &state.recovery_box)
        },
    );
}

/// A key for another identity is told apart before anything is opened, and a
/// wrong key for this one after: the two are different things to tell a
/// person.
#[test]
fn a_recovery_key_is_refused_for_the_reason_it_fails() {
    let mut reasons = std::collections::BTreeMap::new();
    for case in section("recoveries") {
        if expected(&case) != "refused" {
            continue;
        }
        let verified = record(&case);
        let token = text(&case, "token");
        let state = &verified.state;
        let reason = match open_recovery(token, state) {
            Err(error) => error,
            // The package opened: it is the wrapper that was not for it.
            Ok(recovery) => state
                .unwrap_key(&recovery.agreement, &state.recovery_wrap)
                .expect_err("a refused case that opens"),
        };
        let matches = recovery_token_matches(token, &state.account, &state.root);
        assert_eq!(
            matches,
            reason != Error::RecoveryToken,
            "{}: {reason:?}",
            case["name"]
        );
        *reasons.entry(reason.to_string()).or_insert(0) += 1;
    }
    for reason in [
        Error::RecoveryToken,
        Error::RecoveryKey,
        Error::RecoveryIdentity,
        Error::Encoding,
        Error::Unwrap,
    ] {
        assert!(
            reasons.contains_key(&reason.to_string()),
            "no case is refused with {reason:?}: {reasons:?}"
        );
    }
}

fn statements(list: &Value) -> Vec<Signed> {
    let list = list.as_array().expect("a list of statements");
    list.iter().map(signed).collect()
}

/// The private key a case gives for an incoming-share key.
fn inbox_key(owner: &Value, public: &str) -> AgreementKey {
    let private = owner["inboxes"][public]
        .as_str()
        .unwrap_or_else(|| panic!("the case has no private key for {public}"));
    AgreementKey::from_bytes(decode_exact(private).expect("a key"))
}

/// The wrapper in a statement's payload, and the nonce it was sealed with.
fn wrapper_in(statement: &Value) -> (String, [u8; 12]) {
    let raw = decode(text(statement, "payload")).expect("a payload");
    let value: Value = serde_json::from_slice(&raw).expect("JSON");
    let wrapper = value["wrap"].as_str().expect("a wrapper").to_owned();
    let nonce = nonce_of(wrapper.split_once('.').expect("a wrapper").1);
    (wrapper, nonce)
}

/// Runs `with` on what a sender holds of a share: the invitation it made and
/// the secret it sent.
fn with_offer<T>(case: &Value, with: impl FnOnce(&Offer<'_>) -> T) -> T {
    let secret = key(&case["invitation"], "secret");
    with(&Offer {
        share_id: text(&case["share"], "id"),
        invite_id: text(&case["invitation"], "id"),
        sha256: text(&case["share"], "sha256"),
        owner: text(&case["owner"], "account"),
        owner_root: text(&case["owner"], "root"),
        secret: &secret,
    })
}

fn judge_claim(case: &Value) -> Result<Accepted, Error> {
    with_offer(case, |offer| {
        verify_claim(
            &signed(&case["claim"]),
            text(&case["claim"], "proof"),
            offer,
            &statements(&case["recipient"]["history"]),
        )
    })
}

#[test]
fn a_claim_is_granted_or_refused_and_the_grant_is_made_again() {
    run(
        "shares",
        "granted",
        |case| judge_claim(case).map_err(|error| error.to_string()),
        |case, accepted| {
            let owner = &case["owner"];
            let share_key = key(case, "key");
            let claim = signed(&case["claim"]);
            let proof = binding_proof(&key(&case["invitation"], "secret"), &claim)
                .map_err(|error| error.to_string())?;
            same(
                "the proof made again",
                proof.as_str(),
                text(&case["claim"], "proof"),
            )?;

            // The grant, from the one-time key and the nonce it was made with.
            let (wrapper, nonce) = wrapper_in(&case["grant"]);
            let ephemeral = AgreementKey::from_bytes(key(case, "grant_ephemeral"));
            let grant = with_offer(case, |offer| {
                accepted.grant(
                    offer,
                    text(owner, "head"),
                    text(owner, "signer"),
                    &share_key,
                    &ephemeral,
                    nonce,
                )
            })
            .map_err(|error| error.to_string())?;
            same(
                "the wrapper made again",
                grant.wrap.as_str(),
                wrapper.as_str(),
            )?;
            let signer = SigningKey::from_seed(key(owner, "seed"));
            let statement = Signed::sign(Kind::NamedGrant, &grant.encode(), &signer)
                .map_err(|error| error.to_string())?;
            same("the grant made again", statement, signed(&case["grant"]))?;

            // And the recipient opens it, with the key the grant names.
            let recipient = &accepted.recipient;
            let context = named_context(
                text(&case["share"], "sha256"),
                &recipient.account,
                &recipient.root,
                recipient.epoch,
            );
            let opened = unwrap(
                &inbox_key(&case["recipient"], &grant.inbox),
                &grant.wrap,
                &context,
            )
            .map_err(|error| error.to_string())?;
            same("the share's key", opened, share_key)
        },
    );
}

/// Each refusal is for its own reason: a claim that is not the invited
/// person's is told from one whose history has moved on.
#[test]
fn a_claim_is_refused_for_the_reason_it_fails() {
    let mut reasons = std::collections::BTreeMap::new();
    for case in section("shares") {
        if expected(&case) == "refused" {
            let reason = judge_claim(&case).expect_err("a refused case");
            let name = match reason {
                Error::History(_) => "history".to_owned(),
                other => format!("{other:?}"),
            };
            *reasons.entry(name).or_insert(0) += 1;
        }
    }
    for reason in [
        "Claim",
        "ClaimProof",
        "RecipientIdentity",
        "RecipientHistory",
        "ClaimSigner",
        "history",
    ] {
        assert!(
            reasons.contains_key(reason),
            "no case is refused as {reason}: {reasons:?}"
        );
    }
    assert_eq!(reasons.len(), 6, "{reasons:?}");
}

/// The invitation of every case is the sender's own to make: the share's
/// key wrapped to its incoming-share key, in a statement written as the Go
/// CLI writes it.
#[test]
fn an_invitation_is_made_again_and_opens_for_its_sender() {
    let mut made = 0;
    for case in section("shares") {
        let name = &case["name"];
        let owner = &case["owner"];
        let invited = owner["invited"].as_u64().expect("a count") as usize;
        let history = statements(&owner["history"]);
        let then = verify_history(&history[..invited], text(owner, "account"))
            .unwrap_or_else(|error| panic!("{name}: the sender's history: {error}"));
        let share_key = key(&case, "key");
        let sha256 = text(&case["share"], "sha256");
        let (wrapper, nonce) = wrapper_in(&case["invite"]);
        let ephemeral = AgreementKey::from_bytes(key(&case, "invite_ephemeral"));
        let state = &then.state;
        let wrap = state
            .wrap_share_key(sha256, &share_key, &ephemeral, nonce)
            .expect("the sender shares");
        assert_eq!(wrap, wrapper, "{name}");

        let invitation = &case["invitation"];
        assert_eq!(
            commitment(&key(invitation, "secret")),
            text(invitation, "commitment"),
            "{name}"
        );
        let invite = Invite {
            account: state.account.clone(),
            root: state.root.clone(),
            head: then.head.clone(),
            signer: text(owner, "signer").to_owned(),
            sha256: sha256.to_owned(),
            inbox: state.inbox.clone().expect("the sender shares"),
            epoch: state.epoch,
            wrap,
            invitation: Invitation {
                id: text(invitation, "id").to_owned(),
                email: text(invitation, "email").to_owned(),
                commitment: text(invitation, "commitment").to_owned(),
            },
        };
        let signer = SigningKey::from_seed(key(owner, "seed"));
        let statement = Signed::sign(Kind::NamedInvite, &invite.encode(), &signer).unwrap();
        assert_eq!(statement, signed(&case["invite"]), "{name}");

        let context = named_context(sha256, &state.account, &state.root, state.epoch);
        let opened = unwrap(&inbox_key(owner, &invite.inbox), &invite.wrap, &context).unwrap();
        assert_eq!(opened, share_key, "{name}");
        made += 1;
    }
    assert!(made >= 30, "{made} invitations were made again");
}

/// A history written as a client writes it is written back byte for byte:
/// this crate and the Go CLI spell a state, and a vault, the same way.
#[test]
fn a_state_and_a_vault_are_written_as_a_client_writes_them() {
    let file = read("histories.json");
    let statement = |id: &Value| signed(&file["signed"][id.as_str().expect("an id")]);
    let (mut states, mut vaults, mut others) = (0, 0, 0);
    for case in file["cases"].as_array().expect("a list of cases") {
        if expected(case) != "valid" {
            continue;
        }
        let events: Vec<Signed> = case["events"]
            .as_array()
            .expect("a list of events")
            .iter()
            .map(statement)
            .collect();
        let account = text(case, "account");
        let history = verify_history(&events, account).expect("a valid case");
        let last = &events.last().expect("an event").payload;
        let written = encode(&history.state.encode());
        if case["client"] != true {
            // Spelt some other way on purpose; what is read from it is still
            // a state this crate can write.
            others += 1;
            continue;
        }
        assert_eq!(&written, last, "{}", case["name"]);
        states += 1;
        if !case["vault"].is_null() {
            let vault = statement(&case["vault"]);
            let record = Record {
                events,
                vault: vault.clone(),
            };
            let verified = verify_record(&record, account).expect("a valid case");
            assert_eq!(
                encode(&verified.vault.encode()),
                vault.payload,
                "{}",
                case["name"]
            );
            vaults += 1;
        }
    }
    assert!(
        states >= 20 && vaults >= 5 && others >= 5,
        "{states} states, {vaults} vaults, {others} spelt otherwise"
    );
}

/// Given the key and the nonce of an envelope that opens, this crate seals
/// the same envelope: Go's, Web Crypto's and the generator's alike.
#[test]
fn every_envelope_that_opens_is_sealed_again() {
    let cases = read("envelopes.json");
    let mut sealed = 0;
    for case in cases.as_array().expect("a list of cases") {
        if expected(case) != "opens" {
            continue;
        }
        let data = bytes(case, "envelope");
        let nonce: [u8; 12] = data[12..24].try_into().expect("a nonce");
        let made = envelope::encrypt(&bytes(case, "plaintext"), &key(case, "key"), nonce)
            .expect("within the limit");
        assert_eq!(made, data, "{}", case["name"]);
        assert_eq!(encode(&key(case, "key")), text(case, "key"));
        sealed += 1;
    }
    assert!(sealed >= 5, "{sealed} envelopes were sealed again");
}
