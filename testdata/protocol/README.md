# Protocol cases

The envelopes, signed statements, device histories and vault records every
implementation of the protocol is held to. The account is fictional. Every key
and nonce is SHA-256 of a label, so no key here was ever anyone's and the
files are the same on every run.

Three test suites read the files and each must agree with every entry:
[protocol-cases.test.mjs](../../test/protocol-cases.test.mjs) for the viewer's
code, [cases_test.go](../../go/internal/identity/cases_test.go) and
[its counterpart for envelopes](../../go/internal/sealed/cases_test.go) for Go,
and [cases.rs](../../crates/slink-protocol/tests/cases.rs) for Rust.
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
| `rule` | For a history Rust refuses, the rule it broke, as `Refusal::as_str` names it. The Rust test fails on a refusal for any other rule |
| `why` | For a case whose verdicts differ, the reason |

A case with one verdict per implementation records a difference that is known
and decided. In all of them Rust refuses.

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
