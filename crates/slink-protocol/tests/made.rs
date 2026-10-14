//! What this crate makes, it opens again, and what is damaged on the way
//! does not open. The cases in `testdata/protocol/keys.json` are made from
//! keys someone chose; these are made from keys chosen at random, the same
//! ones on every run and on every target the binary is built for.

use slink_protocol::encoding::{decode, encode};
use slink_protocol::{
    AgreementKey, Error, Kind, MAX_DEVICE_NAME, Recovery, Request, Signed, SigningKey,
    code_matches, confirmation, envelope, seal, unseal, unwrap, wrap,
};

/// xorshift64: enough to choose keys nobody will use and where to damage
/// something.
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

    fn array<const N: usize>(&mut self) -> [u8; N] {
        let mut out = [0; N];
        out.iter_mut().for_each(|byte| *byte = self.next() as u8);
        out
    }

    /// Text of up to `limit` characters, some of them outside ASCII and some
    /// that JSON escapes.
    fn text(&mut self, limit: usize) -> String {
        const SOME: &[char] = &[
            'a',
            'Z',
            '0',
            '/',
            ' ',
            '"',
            '\\',
            '<',
            '&',
            '\n',
            '\u{0}',
            '\u{7f}',
            '\u{e9}',
            '\u{2028}',
            '\u{1f4bb}',
        ];
        (0..self.below(limit + 1))
            .map(|_| SOME[self.below(SOME.len())])
            .collect()
    }
}

/// A copy of `text` with one character changed, removed or added.
fn damaged(random: &mut Random, text: &str) -> String {
    const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.=";
    let mut bytes = text.as_bytes().to_vec();
    let at = random.below(bytes.len());
    let other = ALPHABET[random.below(ALPHABET.len())];
    match random.below(3) {
        0 if !bytes.is_empty() => bytes[at] = other,
        1 if !bytes.is_empty() => drop(bytes.remove(at)),
        _ => bytes.insert(at, other),
    }
    String::from_utf8(bytes).expect("ASCII")
}

#[test]
fn a_box_opens_again_and_a_damaged_one_does_not() {
    let mut random = Random(0x5eed_0101);
    let mut refused = 0;
    for _ in 0..1500 {
        let key = random.array();
        let length = random.below(200);
        let plain = random.bytes(length);
        let context = random.text(40);
        let sealed = seal(&key, &plain, &context, random.array());
        assert_eq!(unseal(&key, &sealed, &context).unwrap(), plain);
        assert_eq!(decode(&sealed).unwrap().len(), 12 + plain.len() + 16);

        let changed = damaged(&mut random, &sealed);
        if changed != sealed {
            assert!(unseal(&key, &changed, &context).is_err(), "{changed}");
            refused += 1;
        }
        let other = format!("{context}x");
        assert_eq!(unseal(&key, &sealed, &other), Err(Error::Unseal));
        let mut wrong = key;
        wrong[random.below(32)] ^= 1 << random.below(8);
        assert_eq!(unseal(&wrong, &sealed, &context), Err(Error::Unseal));
    }
    assert!(refused > 1400, "{refused} damaged boxes");
}

#[test]
fn a_wrapper_opens_again_for_its_recipient_and_no_damaged_one_does() {
    let mut random = Random(0x5eed_0102);
    let mut refused = 0;
    for _ in 0..600 {
        let recipient = AgreementKey::from_bytes(random.array());
        let stranger = AgreementKey::from_bytes(random.array());
        let ephemeral = AgreementKey::from_bytes(random.array());
        let key = random.array();
        let context = random.text(60);
        let wrapper = wrap(
            &recipient.public(),
            &key,
            &context,
            &ephemeral,
            random.array(),
        )
        .expect("a key made from random bytes is not of small order");
        assert_eq!(wrapper.len(), 124);
        assert_eq!(unwrap(&recipient, &wrapper, &context).unwrap(), key);
        assert_eq!(
            unwrap(&stranger, &wrapper, &context),
            Err(Error::Unwrap),
            "opened by a key it was not made for"
        );
        assert_eq!(
            unwrap(&recipient, &wrapper, &format!("{context}/")),
            Err(Error::Unwrap)
        );
        for _ in 0..4 {
            let changed = damaged(&mut random, &wrapper);
            if changed != wrapper {
                assert!(unwrap(&recipient, &changed, &context).is_err(), "{changed}");
                refused += 1;
            }
        }
    }
    assert!(refused > 2300, "{refused} damaged wrappers");
}

#[test]
fn an_envelope_opens_again_under_its_key_only() {
    let mut random = Random(0x5eed_0103);
    for _ in 0..300 {
        let key = random.array();
        let length = random.below(2000);
        let plain = random.bytes(length);
        let data = envelope::encrypt(&plain, &key, random.array()).unwrap();
        assert_eq!(data.len(), envelope::HEADER_SIZE + plain.len() + 16);
        assert_eq!(envelope::decrypt(&data, &encode(&key)).unwrap(), plain);
        let mut changed = data.clone();
        let at = random.below(changed.len());
        changed[at] ^= 1 << random.below(8);
        assert!(envelope::decrypt(&changed, &encode(&key)).is_err());
        let other: [u8; 32] = random.array();
        assert_eq!(
            envelope::decrypt(&data, &encode(&other)),
            Err(Error::Decrypt)
        );
    }
}

#[test]
fn a_statement_verifies_as_what_it_was_signed_as_and_nothing_else() {
    let mut random = Random(0x5eed_0104);
    for round in 0..200 {
        let key = SigningKey::from_seed(random.array());
        let other = SigningKey::from_seed(random.array());
        let length = random.below(300);
        let payload = random.bytes(length);
        let kind = Kind::ALL[round % Kind::ALL.len()];
        let signed = Signed::sign(kind, &payload, &key).unwrap();
        assert_eq!(signed.read(kind).unwrap(), payload);
        signed.verify(kind, &key.public()).unwrap();
        assert_eq!(
            signed.verify(kind, &other.public()),
            Err(Error::Signature(kind))
        );
        for offered in Kind::ALL {
            if offered != kind {
                assert_eq!(
                    signed.verify(offered, &key.public()),
                    Err(Error::Signature(offered))
                );
            }
        }
        // The same key and payload sign the same way twice.
        assert_eq!(Signed::sign(kind, &payload, &key).unwrap(), signed);
    }
}

fn request(random: &mut Random, key: &SigningKey) -> Request {
    let mut name = random.text(30);
    while name.len() > MAX_DEVICE_NAME {
        name.pop();
    }
    Request {
        account: format!("usr_{}", random.below(1000)),
        root: SigningKey::from_seed(random.array()).public(),
        name,
        sign_key: key.public(),
        box_key: AgreementKey::from_bytes(random.array()).public(),
        nonce: encode(&random.array::<24>()),
        created: random.next() >> 11,
    }
}

#[test]
fn a_request_reads_back_whatever_its_name_holds() {
    let mut random = Random(0x5eed_0105);
    for _ in 0..500 {
        let key = SigningKey::from_seed(random.array());
        let made = request(&mut random, &key);
        let signed = Signed::sign(Kind::Request, &made.encode(), &key).unwrap();
        assert_eq!(Request::verify(&signed).unwrap(), made, "{:?}", made.name);
        let code = confirmation(&signed).unwrap();
        assert!(code_matches(&code, &signed));
        // What is written is JSON any reader takes.
        let read: serde_json::Value = serde_json::from_slice(&made.encode()).unwrap();
        assert_eq!(read["name"], made.name.as_str());
        assert_eq!(read["created"], made.created);
    }
}

#[test]
fn a_damaged_request_is_refused_or_still_names_its_signer() {
    let mut random = Random(0x5eed_0106);
    let key = SigningKey::from_seed(random.array());
    let original = request(&mut random, &key).encode();
    let (mut accepted, mut refused) = (0, 0);
    // Characters JSON gives meaning to, and two that are not text.
    const MEANINGFUL: &[u8] = b"{}[]\",:\\0129-.eEntf \n\x00\xff";
    for _ in 0..4000 {
        let mut raw = original.clone();
        for _ in 0..=random.below(2) {
            if raw.is_empty() {
                raw.push(b'{');
            }
            let at = random.below(raw.len());
            match random.below(4) {
                0 => raw[at] ^= 1 << random.below(8),
                1 => drop(raw.remove(at)),
                2 => raw.insert(at, MEANINGFUL[random.below(MEANINGFUL.len())]),
                _ => raw.truncate(at),
            }
        }
        let Ok(signed) = Signed::sign(Kind::Request, &raw, &key) else {
            continue;
        };
        match Request::verify(&signed) {
            Ok(request) => {
                accepted += 1;
                // Signed by the key it names, which is still this one, and
                // of a shape the service stores.
                assert_eq!(request.sign_key, key.public());
                assert!(request.name.len() <= MAX_DEVICE_NAME);
                assert_eq!(decode(&request.box_key).unwrap().len(), 32);
                assert_eq!(decode(&request.nonce).unwrap().len(), 24);
            }
            Err(Error::Request | Error::Signature(_) | Error::SigningKey) => refused += 1,
            Err(other) => panic!("a request failed as {other:?}"),
        }
    }
    // Damage to the name, the account or the time leaves a request that is
    // still one; nearly everything else does not.
    assert!(
        refused > 2500 && accepted > 0,
        "{accepted} accepted, {refused} refused"
    );
}

#[test]
fn a_recovery_package_opens_with_its_key_and_no_other_text() {
    use slink_protocol::{
        Device, Operation, State, device_id, open_recovery, recovery_token, recovery_token_matches,
    };
    let mut random = Random(0x5eed_0107);
    for _ in 0..200 {
        let recovery = Recovery {
            root: SigningKey::from_seed(random.array()),
            agreement: AgreementKey::from_bytes(random.array()),
        };
        let secret = random.array();
        let account = format!("usr_{}", random.below(1000));
        let vault_key = random.array();
        let device = AgreementKey::from_bytes(random.array());
        let sign = SigningKey::from_seed(random.array());
        let mut state = State {
            account: account.clone(),
            seq: 0,
            prev: String::new(),
            kind: Operation::Init,
            signer: "root".into(),
            root: recovery.root.public(),
            recovery: recovery.agreement.public(),
            recovery_box: recovery.seal(&account, &secret, random.array()),
            recovery_wrap: String::new(),
            epoch: 1 + random.below(5) as u64,
            devices: Vec::new(),
            inbox: None,
        };
        state.recovery_wrap = state
            .wrap_key(
                &state.recovery,
                &vault_key,
                &AgreementKey::from_bytes(random.array()),
                random.array(),
            )
            .unwrap();
        let wrap = state
            .wrap_key(
                &device.public(),
                &vault_key,
                &AgreementKey::from_bytes(random.array()),
                random.array(),
            )
            .unwrap();
        state.devices.push(Device {
            id: device_id(&sign.public(), &device.public()),
            name: random.text(10),
            sign_key: sign.public(),
            box_key: device.public(),
            wrap,
        });

        let token = recovery_token(&account, &state.root, &secret);
        let opened = open_recovery(&token, &state).unwrap();
        assert_eq!(opened.root.seed(), recovery.root.seed());
        assert_eq!(
            state
                .unwrap_key(&opened.agreement, &state.recovery_wrap)
                .unwrap(),
            vault_key
        );
        assert_eq!(
            state.unwrap_key(&device, &state.devices[0].wrap).unwrap(),
            vault_key
        );
        // The device's wrapper is not the recovery identity's, nor the other
        // way round, and neither opens at another epoch.
        assert!(
            state
                .unwrap_key(&opened.agreement, &state.devices[0].wrap)
                .is_err()
        );
        let next = State {
            epoch: state.epoch + 1,
            ..state.clone()
        };
        assert!(next.unwrap_key(&device, &state.devices[0].wrap).is_err());

        let changed = damaged(&mut random, &token);
        if changed.trim() != token {
            assert!(open_recovery(&changed, &state).is_err(), "{changed}");
        }
        assert!(recovery_token_matches(&token, &account, &state.root));
        // The state as written reads back as JSON any reader takes.
        let read: serde_json::Value = serde_json::from_slice(&state.encode()).unwrap();
        assert_eq!(read["devices"][0]["name"], state.devices[0].name.as_str());
        assert_eq!(read["epoch"], state.epoch);
        assert!(read.get("inbox").is_none());
    }
}

/// An account made with this crate alone: its keys, its history as signed
/// statements, and the state the history leaves.
struct Account {
    device: slink_protocol::SigningKey,
    device_id: String,
    inbox: AgreementKey,
    events: Vec<Signed>,
    state: slink_protocol::State,
    head: String,
}

/// A first event by the recovery identity and a second, by the device, that
/// turns sharing on. Each is checked by the verifier as it is added.
fn account(random: &mut Random, name: &str) -> Account {
    use slink_protocol::{Device, Operation, State, device_id, verify_history};
    let recovery = Recovery {
        root: SigningKey::from_seed(random.array()),
        agreement: AgreementKey::from_bytes(random.array()),
    };
    let device = SigningKey::from_seed(random.array());
    let device_box = AgreementKey::from_bytes(random.array());
    let vault_key: [u8; 32] = random.array();
    let mut state = State {
        account: name.to_owned(),
        seq: 0,
        prev: String::new(),
        kind: Operation::Init,
        signer: "root".into(),
        root: recovery.root.public(),
        recovery: recovery.agreement.public(),
        recovery_box: recovery.seal(name, &random.array(), random.array()),
        recovery_wrap: String::new(),
        epoch: 1,
        devices: Vec::new(),
        inbox: None,
    };
    let one_time = |random: &mut Random| AgreementKey::from_bytes(random.array());
    state.recovery_wrap = state
        .wrap_key(
            &state.recovery,
            &vault_key,
            &one_time(random),
            random.array(),
        )
        .unwrap();
    let id = device_id(&device.public(), &device_box.public());
    let wrap = state
        .wrap_key(
            &device_box.public(),
            &vault_key,
            &one_time(random),
            random.array(),
        )
        .unwrap();
    state.devices.push(Device {
        id: id.clone(),
        name: random.text(12),
        sign_key: device.public(),
        box_key: device_box.public(),
        wrap,
    });
    let first = Signed::sign(Kind::Event, &state.encode(), &recovery.root).unwrap();
    let history = verify_history(std::slice::from_ref(&first), name).expect("a first event");
    assert_eq!(history.state, state);

    let inbox = AgreementKey::from_bytes(random.array());
    let mut next = history.state.clone();
    next.kind = Operation::Sharing;
    next.signer = id.clone();
    next.seq = 1;
    next.prev = history.head;
    next.inbox = Some(inbox.public());
    let second = Signed::sign(Kind::Event, &next.encode(), &device).unwrap();
    let events = vec![first, second];
    let history = verify_history(&events, name).expect("sharing turned on");
    assert_eq!(history.state, next);
    Account {
        device,
        device_id: id,
        inbox,
        events,
        state: history.state,
        head: history.head,
    }
}

#[test]
fn a_share_goes_from_invitation_to_grant_between_accounts_made_here() {
    use slink_protocol::{
        Claim, Invitation, Invite, Offer, binding_proof, commitment, named_context, verify_claim,
    };
    let mut random = Random(0x5eed_0108);
    for round in 0..40 {
        let sender = account(&mut random, &format!("usr_sender_{round}"));
        let recipient = account(&mut random, &format!("usr_recipient_{round}"));
        let share_key: [u8; 32] = random.array();
        let sha256 = "ab".repeat(32);
        let secret: [u8; 32] = random.array();
        let invitation = Invitation {
            id: encode(&random.array::<24>()),
            email: "recipient@example.com".into(),
            commitment: commitment(&secret),
        };
        let own = sender
            .state
            .wrap_share_key(
                &sha256,
                &share_key,
                &AgreementKey::from_bytes(random.array()),
                random.array(),
            )
            .unwrap();
        let invite = Invite {
            account: sender.state.account.clone(),
            root: sender.state.root.clone(),
            head: sender.head.clone(),
            signer: sender.device_id.clone(),
            sha256: sha256.clone(),
            inbox: sender.inbox.public(),
            epoch: 1,
            wrap: own.clone(),
            invitation: invitation.clone(),
        };
        Signed::sign(Kind::NamedInvite, &invite.encode(), &sender.device)
            .unwrap()
            .verify(Kind::NamedInvite, &sender.device.public())
            .unwrap();
        let context = |state: &slink_protocol::State| {
            named_context(&sha256, &state.account, &state.root, state.epoch)
        };
        assert_eq!(
            unwrap(&sender.inbox, &own, &context(&sender.state)).unwrap(),
            share_key
        );

        let claim = Claim {
            account: recipient.state.account.clone(),
            root: recipient.state.root.clone(),
            head: recipient.head.clone(),
            signer: recipient.device_id.clone(),
            share_id: "sharefxtvr2345".into(),
            invite_id: invitation.id.clone(),
            sha256: sha256.clone(),
            owner: sender.state.account.clone(),
            owner_root: sender.state.root.clone(),
            inbox: recipient.inbox.public(),
            epoch: 1,
        };
        let signed =
            Signed::sign(Kind::RecipientClaim, &claim.encode(), &recipient.device).unwrap();
        let proof = binding_proof(&secret, &signed).unwrap();
        let offer = Offer {
            share_id: "sharefxtvr2345",
            invite_id: &invitation.id,
            sha256: &sha256,
            owner: &sender.state.account,
            owner_root: &sender.state.root,
            secret: &secret,
        };
        let accepted = verify_claim(&signed, &proof, &offer, &recipient.events).unwrap();
        assert_eq!(accepted.claim, claim);
        let grant = accepted
            .grant(
                &offer,
                &sender.head,
                &sender.device_id,
                &share_key,
                &AgreementKey::from_bytes(random.array()),
                random.array(),
            )
            .unwrap();
        assert_eq!(grant.claim_hash, signed.hash(Kind::RecipientClaim).unwrap());
        assert_eq!(
            unwrap(&recipient.inbox, &grant.wrap, &context(&recipient.state)).unwrap(),
            share_key
        );
        // The grant is the recipient's alone: not the sender's to open, nor
        // anyone's under the sender's context.
        assert!(unwrap(&sender.inbox, &grant.wrap, &context(&recipient.state)).is_err());
        assert!(unwrap(&recipient.inbox, &grant.wrap, &context(&sender.state)).is_err());

        // The same claim with another secret, for another share, or against
        // the sender's own history, releases nothing.
        let other: [u8; 32] = random.array();
        let forged = binding_proof(&other, &signed).unwrap();
        assert_eq!(
            verify_claim(&signed, &forged, &offer, &recipient.events),
            Err(Error::ClaimProof)
        );
        let elsewhere = Offer {
            share_id: "sharefxtvr2346",
            ..offer
        };
        assert_eq!(
            verify_claim(&signed, &proof, &elsewhere, &recipient.events),
            Err(Error::ClaimProof)
        );
        assert!(verify_claim(&signed, &proof, &offer, &sender.events).is_err());
        assert_eq!(
            verify_claim(&signed, &proof, &offer, &recipient.events[..1]),
            Err(Error::RecipientIdentity)
        );
    }
}
