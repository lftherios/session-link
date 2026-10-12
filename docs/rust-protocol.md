# Rust envelope and signed-history verification

2026-10-12 · Implemented slice of the [Rust migration](rust-migration.md), backlog item 5 · `crates/slink-protocol` opens envelopes and verifies signed statements, device histories and vault records; Go, the viewer and Rust agree on 341 shared cases with 38 recorded differences; the first version passed on all six targets and the sweep on Linux in CI, and the two rules added since have run locally.

What the Rust side of the protocol is so far, how it is held to the three
verifiers that already exist, and what comparing them found. The crate checks
and does not yet make anything: wrapping, recovery, named-share statements,
sealing and signing are the rest of the plan's P1 line and are listed at the
end. It has no caller in the product; `slink-rs build-check` links and runs it
so that every target proves it.

[identity-encryption.md](identity-encryption.md) describes the protocol
itself. Two of the findings below became rules in it, in every implementation
at once: a signing key of small order is refused, and a device name holds
whole characters only.

## The crate

[crates/slink-protocol](../crates/slink-protocol) is deterministic and holds
no state. Every function takes its input as untrusted and returns what it
verified or one error, never part of a result.

| Function | What it does |
| --- | --- |
| `envelope::validate` | Size, magic, version and reserved bytes of an encrypted document |
| `envelope::decrypt` | The document, if the key opens the envelope with its header as associated data |
| `Signed::verify` | Whether a key signed a payload as one of the seven kinds of statement |
| `verify_history` | An account's device history from its first event: the last state and the hash that names it |
| `verify_record` | The history, then that its vault was written for that state by someone the state lets write it |

A history that fails reports the one message the Go CLI prints, and carries
the name of the rule it broke. Go and the viewer have no such name; it is
there so that a case can say which rule it is about.

## Choices

| Area | Choice | Reason |
| --- | --- | --- |
| Cryptography | ring 0.17, named only in [crypto.rs](../crates/slink-protocol/src/crypto.rs) | TLS already links it, so the crate adds no package. On every signature the cases and the sweep try it decides as Go and OpenSSL do, and on the cases as BoringSSL does. One file to replace if the WASM build at P8 wants another library |
| Weak keys | A list of the points of small order, checked before the library is asked | No library here refuses them; see the seventh finding |
| Base64 | The `base64` crate's strict engine | A key, a payload and a signature each have one spelling: no padding, no second alphabet, no stray bits, no white space |
| Payload JSON | serde_json through the crate's own reader, [payload.rs](../crates/slink-protocol/src/payload.rs) | A field is taken by its exact name and in its exact type, nothing is filled in, and no object gives a key twice. See the first, second and sixth findings |
| Sizes | The hosted service's: 32 KiB of text for an event, 8 MiB for a vault statement | Nothing the service stores is refused, and nothing longer is ever decoded |
| Statement kinds | An enum of the seven | The kind is part of what is signed, and a kind cannot be mistyped into one that verifies |
| Error text | The Go CLI's | The differential runner compares what the two binaries print |

## Shared cases

[testdata/protocol](../testdata/protocol/README.md) holds the cases as data,
and each language's own tests read them. Before this, the one shared vector
was a Web Crypto envelope, and the two history verifiers were held to each
other by one rule, tested once in each language with keys made at random on
each run.

| File | Cases |
| --- | --- |
| `envelopes.json` | 55: envelopes sealed by Web Crypto, by Go and by `node:crypto`; one bit changed in each part; cut short and lengthened; eight spellings of a key; five that authenticate under a header no reader supports |
| `statements.json` | 54: each kind as itself and offered as another; one spelling of a payload, a signature and a key; the eight points of small order as keys, in all 14 spellings |
| `histories.json` | 232: 34 histories and records every implementation accepts, with the state each must report; 160 that all refuse, each naming its rule; 38 recorded differences |

A case needs signatures, so the files are written by
[protocol-cases.mjs](../scripts/protocol-cases.mjs) and not by hand. Every key
and nonce is derived from a label, so the output never changes and no key in
it was ever anyone's. The script signs, seals and wraps with `node:crypto`
directly, which makes it a fourth reading of the wire format and not a copy of
one under test; the viewer's test opens a wrapper and a vault from the cases
to show the script wrote what a client writes.

Every rule the Rust crate names has at least one case, and the Rust test
accepts a refusal only for the rule the case is about. A case that is refused
for some other reason fails.

## Differential sweep

[protocol-diff.mjs](../scripts/protocol-diff.mjs) looks for differences no
case names. It takes eight valid histories and five records and changes one
thing at a time in the last event, signed again by its rightful key: each
field removed, given each JSON type, respelt as a number or a string, renamed
or repeated; the payload's text around the JSON; every combination of
operation, signer, epoch, incoming-share key, device list and recovery wrapper
that could follow three of the states; every operation in every name under
every key; each character of a signed text; every byte and every length of
two envelopes; and the eight points of small order in their spellings. It
adds the limits, which need a megabyte of history and so are not in a file:
512 events and 513, 32 devices and 33.

On 2026-10-12 it ran 23,503 cases in under 40 seconds. JavaScript, Go and Rust
judged 21,521 alike, and where all three accepted they reported the same state
at the same head. The other 1,982 are the kinds of recorded difference below.
Anything else fails the run, as does any case Rust accepts and another
implementation refuses. The `protocol` job in `rust.yml` runs it on Linux,
where the first run, at commit `46fd4a5`, gave the same counts.

With `--server` and a checkout of the hosted service it runs that verifier as
a fourth. On the same cases the service refused nothing Rust accepted, and
accepted 1,200 that Rust refused, all of them among the kinds below.

[check-protocol-browser.mjs](../scripts/check-protocol-browser.mjs) runs the
case files through the viewer's code in a browser, because Node's Web Crypto
is OpenSSL and a browser brings its own. Headless Chromium 149 gave all 341
cases the verdict recorded for the viewer.

## What the comparison found

1. **One signed payload, two states.** Go's `encoding/json` matches a field's
   name without regard to case, the long s and the Kelvin sign included, and
   leaves a field as it was when it meets `null`. JavaScript takes the exact
   name and the last value. So a device written with `"name":"Laptop"` and
   then `"Name":"Other"` is called Laptop by the viewer and the service and
   Other by the Go CLI, and an event with an incoming-share key followed by
   `"inbox":null` shares for Go and does not for JavaScript. Both verify the
   same signature. Writing such an event takes a signing key of the account,
   so it hands nobody a key; what it breaks is that everyone who verifies a
   history reads the same one. Rust refuses a payload with a field's name in
   another case or with any key given twice.
2. **Go fills in what is missing.** A field that is `null` or left out reads
   as empty or zero, so a first event with no sequence number, a device with
   no name and a vault with no ciphertext all pass. JavaScript refuses them.
3. **Go measures wrappers and JavaScript decodes them.** For a device's
   wrapper, the recovery wrapper and the recovery box, Go checks the length.
   It checks a vault's ciphertext only when it opens it.
4. **JavaScript reads numbers and text loosely.** `1.0`, `1e0` and `10e-1`
   are counts, an incoming-share key of `false` is no key, and the viewer's
   decoder drops a byte-order mark that the service's does not. Past 2^53 it
   is Go that accepts a count JavaScript cannot hold.
5. **Only the service limits size.** It refuses an event of more than 32 KiB
   and a vault statement of more than 8 MiB, as text. Go and the viewer rely
   on it.
6. **serde reads a struct from a list.** A derived `Deserialize` accepts a
   JSON array of the fields in order, and nothing else reads a statement that
   way. The crate reads payloads by name only, and two cases pin it.
7. **Every library takes a public key of small order.** Go, OpenSSL in Node
   22, BoringSSL in Chromium 149 and ring all verify under one. For the
   identity point a single fixed signature fits every payload, so a history
   whose root key was that point verified and could be continued by anyone. No
   client makes such a key. Since 2026-10-12 all four implementations refuse
   to check anything under one: each holds the same list of the eight points,
   by the seven numbers their y-coordinate can be spelt with, and compares a
   key against it with the sign bit cleared. The 14 spellings are cases, each
   on a payload the library alone would have taken. A device with such a key
   can still be enrolled; nothing it signs, and nothing signed in its name,
   is accepted. All four libraries already refused a signature with the group
   order added and a signature point spelt past the field's size.
8. **Half an emoji in a device name stopped the Go CLI.** `JSON.stringify`
   writes an unpaired surrogate as an escape. The viewer and the service kept
   it; Go reads U+FFFD and writes that into the next event, which the service
   then refused as a renamed device. Since 2026-10-12 the viewer writes such a
   name with U+FFFD in place of the half, and the viewer and the service
   refuse a history or a device request whose name holds one, so no name of
   this kind can be stored. Go is unchanged, because it cannot tell the escape
   from the character after decoding, and Rust refuses the escape anywhere in
   a payload.

## Recorded differences

In every one Rust refuses. Across the sweep nothing Rust accepts is refused by
Go, the viewer or the service, and where all accept they read the same state.

| Payload | JavaScript | Go | Rust |
| --- | --- | --- | --- |
| A field's name in another case | not that field | that field | refused |
| A key given twice | the last | the last, or the one before a `null` | refused |
| A field that is `null` or left out | refused | empty or zero | refused |
| A wrapper, recovery box or vault ciphertext that has the length and does not decode | refused | accepted | refused |
| A count written `1.0`, `1e0` or `10e-1` | accepted | refused | refused |
| A count past 2^53 | refused | accepted | refused |
| An incoming-share key of `false` or `0` | no key | refused | refused |
| A byte-order mark first | accepted by the viewer, refused by the service | refused | refused |
| Bytes that are not UTF-8 | refused by the viewer, U+FFFD to the service | U+FFFD | refused |
| Half a surrogate pair as an escape, in a device name | refused | U+FFFD | refused |
| A count of `-0` | zero | zero | refused |
| Half a surrogate pair as an escape, anywhere else | kept | U+FFFD | refused |
| 128 or more levels of nesting | accepted | accepted | refused |
| A number too large for a double, in a field no verifier reads | accepted | accepted | refused |
| A statement past the service's size limit | accepted by the viewer, refused by the service | accepted | refused |

In the second row and the last five, Rust refuses what both others read: a
repeated key by decision, four from serde_json and one from the size limit. No
client writes any of them.

Apart from the two rules of the seventh and eighth findings, the Go and
JavaScript verifiers are unchanged. The first finding is a difference between
the two that are in use today; Go is replaced at P5 and the viewer's rules at
P8, and whether to tighten either before then is a decision this slice does
not take.

## Failing safely

Beside the cases, [malformed.rs](../crates/slink-protocol/tests/malformed.rs)
damages valid events and vaults at random and signs each with the fixture key,
so that the damage reaches the code behind the signature check. Ed25519 is
deterministic, and the test first shows that ring, given a label's seed,
writes the very signature Node wrote into the case file.

- Of 12,000 damaged and correctly signed events and vaults, each is refused
  with a history or vault error, or is still a statement for the same account
  at the same place in the chain. Nothing panics.
- Text that was never signed is refused, whatever it holds.
- Changing any one character of a signed text makes the record fail, for
  each character of one event and one vault and their signatures.
- An event nested as deep as its size allows, and a vault of a million
  levels, are refused by the parser's own limit; an oversized statement is
  refused before it is decoded.
- Of 10,000 damaged envelopes, none opens.

These run on every target, unlike the sweep, which needs Node and Go.

## Cost

Measured on macOS arm64 with the release profile, through `slink-rs`.

| Build | Binary |
| --- | --- |
| Before this slice | 7,013,488 bytes |
| With the crate and its `build-check` step | 7,030,032 bytes |

The 16 KiB is nearly all the two envelopes and the statement cases that
`build-check` embeds; the cryptography was already linked for TLS. The
lockfile gained no third-party package and holds 213 with the crate itself.

The CI run at `46fd4a5` built each target with the crate linked in, and every
release binary ran `build-check` with its protocol step.

| Target | Before | With the crate |
| --- | --- | --- |
| `aarch64-apple-darwin` | 7,013,504 | 7,030,064 |
| `x86_64-apple-darwin` | 7,442,216 | 7,470,936 |
| `x86_64-unknown-linux-musl` | 8,384,928 | 8,421,792 |
| `aarch64-unknown-linux-musl` | 7,295,440 | 7,295,440 |
| `x86_64-pc-windows-msvc` | 8,529,920 | 8,560,128 |
| `aarch64-pc-windows-msvc` | 7,524,864 | 7,559,168 |

## Not done

- The rest of the plan's P1 line: wrapping and unwrapping a key, the recovery
  box, device requests and their confirmation code, and the named-share
  statements with the recipient's binding proof. The generator already writes
  real wrappers and vaults for them.
- Making anything: sealing an envelope, signing a statement. Those need
  randomness and keys, which the plan keeps apart from the rules here.
- Reading a record off the wire, where the service refuses a signed object
  with a field besides its payload and signature, and pinning a root and a
  head, which belong to the client.
- The limits on the accepting side run only in the sweep. `cargo test` shows
  that 513 events and an oversized statement are refused, not that 512 events
  and 32 devices are accepted.
- Firefox and Safari, whose Web Crypto is neither OpenSSL nor BoringSSL.
- The plan's independent review of the protocol port, which is a P3 line.
