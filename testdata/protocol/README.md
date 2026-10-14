# Protocol cases

The envelopes, signed statements, device histories, vault records, boxes, key
wrappers, device requests and recovery keys every implementation of the
protocol is held to. The accounts are fictional. Every key and nonce is
SHA-256 of a label, so no key here was ever anyone's and the files are the
same on every run. `keys.json` holds private keys in the open for that
reason: each is the hash of a phrase such as `usr_fixture/laptop/box`.

Three test suites read the files and each must agree with every entry:
[protocol-cases.test.mjs](../../test/protocol-cases.test.mjs) for the viewer's
code; [cases_test.go](../../go/internal/identity/cases_test.go),
[keys_test.go](../../go/internal/identity/keys_test.go) and
[the one for envelopes](../../go/internal/sealed/cases_test.go) for Go; and
[cases.rs](../../crates/slink-protocol/tests/cases.rs) and
[keys.rs](../../crates/slink-protocol/tests/keys.rs) for Rust.
[check-protocol-browser.mjs](../../scripts/check-protocol-browser.mjs) runs
the viewer's side again in a browser.
[rust-protocol.md](../../docs/rust-protocol.md) has the decisions behind the
entries that differ.

## envelopes.json

| Field | Meaning |
| --- | --- |
| `name` | What the case is, unique in the file |
| `envelope` | The bytes, in standard base64 |
| `key` | The content key as it would sit in a link |
| `expect` | `opens` or `refused` |
| `plaintext` | For one that opens, the document in standard base64 |

Two of the envelopes were sealed by the implementations themselves and are
kept in [testdata/encryption](../encryption): `webcrypto.json` by the viewer
in a browser and `go.json` by the Go CLI. The cases derived from them change
one thing each.

## statements.json

A `kind`, a `public` key, a `payload` and a `signature`, all as they travel,
and `expect`: `verified` or `refused`. A case with a `why` records something
about the Ed25519 libraries that is worth knowing before changing one.

## histories.json

`signed` holds each statement once, under an id made from its bytes. `cases`
name their events by id.

| Field | Meaning |
| --- | --- |
| `name` | What the case is, unique in the file |
| `account` | The account the history is verified for |
| `events` | The history, as ids |
| `vault` | The vault statement's id, when the case is a whole record |
| `expect` | `valid` or `invalid`, or an object with one verdict each for `js`, `go` and `rust` |
| `state` | For a history that is accepted, what the verifier must report: the head, the last event's fields, the devices by id and name, and the vault's revision. One object, or one each for `js` and `go` where they read different states |
| `client` | `true` where every statement in the case is spelt as a client spells it. Go and Rust must then write the state they read, and the vault, back to the same bytes |
| `rule` | For a history Rust refuses, the rule it broke, as `Refusal::as_str` names it. The Rust test fails on a refusal for any other rule |
| `why` | For a case whose verdicts differ, the reason |

A case with one verdict per implementation records a difference that is known
and decided. In all of them Rust refuses.

## keys.json

What a client makes and opens, in eight lists. A case gives the secrets it
was made with, so an implementation that is handed its randomness can make it
again and must then write the same bytes; Rust does. Go and the viewer choose
their own nonces, so they open each case and sign again where a signature has
no randomness in it. Keys, nonces and wrappers are base64url as they travel;
`plaintext` and `contents` are standard base64.

| List | A case holds | `expect` |
| --- | --- | --- |
| `boxes` | `key`, `context` and the `box`; the nonce is the box's first 12 bytes | `opens` to `plaintext`, or `refused` |
| `wrappers` | The recipient's `private` and `public` key, `context` and the `wrapper`; `ephemeral` is the one-time private key where the case was made the way a client makes one | `opens` to `key`, or `refused` |
| `recipients` | A `public` key to wrap to | `wraps`, or `refused` for a key nothing may be wrapped to |
| `signatures` | `kind`, `seed`, `public`, `payload` and `signature` | None: the seed and the payload have that one signature |
| `requests` | A device request as `payload` and `signature`; `seed` where it is what a client writes | `verified` with the device's `id` and its `code`, or `refused`, or one verdict each |
| `devices` | A record as `account`, `events` and `vault`, the `device` that opens it and its `private` agreement key | `opens` to `vault_key` and `contents`, or `refused` |
| `recoveries` | A record, and the recovery key as `token` | `opens` to the `root` seed, the `agreement` key, `vault_key` and `contents`, or `refused` |
| `shares` | What a sender holds when a recipient's claim arrives: the `share`, its `key`, the `invitation` with its `secret`, the sender as `owner` with its history and the `invite` it signed, the `claim` with its `proof`, and the `recipient`'s history | `granted` with the `grant`, or `refused`, or one verdict each for `go` and `rust` |

In `shares`, `owner.invited` is how many events the sender's history held
when it signed the invitation, `invite_ephemeral` and `grant_ephemeral` are
the one-time private keys of the two wrappers, and `inboxes` gives each
side's incoming-share private keys by their public halves. The viewer has no
sender's side, so it has no verdict on a claim: it opens the shares that are
granted, as their recipient, and every share as its sender.

Every record in `devices` and `recoveries` verifies. What its case is about
comes after: whether the key reaches the vault. A vault's ciphertext, a
recovery package and a wrapper are not opened to verify a history, so a
history can be valid and hold one that nothing opens.

## Changing them

The files are written by
[protocol-cases.mjs](../../scripts/protocol-cases.mjs), because a case needs
signatures. Add or change a case there, then run it with `--update` and commit
the files; `--check` is what CI runs. A change to a rule in any implementation
gets its case in the same change, and a new difference gets its `why` and a
line in [rust-protocol.md](../../docs/rust-protocol.md).

The files are ASCII. Most payloads decode to plain JSON; a few hold bytes that
are not text, which is their point. Go caches a test result without noticing a
change to a file outside its module, so run its tests with `-count=1`.
[protocol-diff.mjs](../../scripts/protocol-diff.mjs) is the way to look for
differences no case names yet, and it also covers the limits, which take a
megabyte of history to reach and so are not stored here.
