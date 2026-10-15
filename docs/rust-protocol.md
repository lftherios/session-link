# Rust protocol crate: verifying and making

2026-10-12, extended 2026-10-13 · Implemented slices of the [Rust migration](rust-migration.md), backlog item 5 and the first part of item 8 · `crates/slink-protocol` opens envelopes and verifies signed statements, device histories and vault records, and makes what a client makes: keys, boxes, wrappers, signatures, device requests, the recovery package and a named share's statements. Go, the viewer and Rust agree on 539 shared cases with 53 recorded differences. Both slices have passed on all six targets and the sweep on Linux in CI; for the making slice, each target's release binary made and opened a wrapper in `build-check`.

What the Rust side of the protocol is so far, how it is held to the three
implementations that already exist, and what comparing them found. The first
slice checked and made nothing. The second, of 2026-10-13, makes everything a
client makes and adds the one check a sender runs before it releases a
share's key. What is left is listed at the end. The crate has no caller in
the product; `slink-rs build-check` links and runs it so that every target
proves it.

[identity-encryption.md](identity-encryption.md) describes the protocol
itself. Two of the findings below became rules in it, in every implementation
at once: a signing key of small order is refused, and a device name holds
whole characters only.

## The crate

[crates/slink-protocol](../crates/slink-protocol) is deterministic and holds
no state. Every function takes its input as untrusted and returns what it
verified or made, or one error, never part of a result.

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

What it makes, and the opening that goes with each:

| Function | What it does |
| --- | --- |
| `SigningKey`, `AgreementKey` | The two kinds of private key: a device has one of each, so has the recovery identity, and an incoming-share key is the second alone. Each gives its public half |
| `Signed::sign` | A statement of one kind over the bytes given, refused when it is longer than its kind may be |
| `envelope::encrypt` | A document in an envelope |
| `seal`, `unseal` | A box: bytes under a key, bound to a context |
| `wrap`, `unwrap` | A wrapper: a 32-byte key that the holder of one X25519 key can open |
| `State::encode`, `Vault::encode`, `Request::encode` | A statement's payload, byte for byte as the Go CLI writes it |
| `State::wrap_key`, `State::unwrap_key`, `State::seal_vault`, `Verified::open_vault` | The vault key and the vault, bound to the account, root key and epoch of a state. Only a record that verified has a vault to open |
| `Recovery::seal`, `open_recovery`, `recovery_token` | The recovery package in a history, and the key a person holds for it |
| `Request::verify`, `confirmation`, `code_matches` | A device's request to join, and the code that confirms it |
| `Invite::encode`, `Claim::encode`, `Grant::encode`, `State::wrap_share_key` | A named share's three statements, and its key wrapped to an account's incoming-share key |
| `verify_claim`, `Accepted::grant` | Whether a recipient's claim is the invited person's, made by a device their history approved; then the grant that answers it |
| `binding_proof`, `commitment` | The proof that ties a claim to an invitation's secret, and what an invitation says of that secret |

A function that makes something is handed its randomness: a key, a nonce, a
one-time key. Given the same ones it writes the same bytes, which is how it is
held to the other implementations. The client will take them from the
operating system. A clock is handed in the same way: `Request::pending` is
told the time.

[keys.rs](../crates/slink-protocol/src/keys.rs),
[wrap.rs](../crates/slink-protocol/src/wrap.rs),
[recovery.rs](../crates/slink-protocol/src/recovery.rs),
[request.rs](../crates/slink-protocol/src/request.rs) and
[named.rs](../crates/slink-protocol/src/named.rs) are the new files. Every
string that binds a box to its purpose is in the crate, so a client never
writes one.

`verify_claim` is the one function here that releases something: nothing of
a share's key is wrapped for a recipient until it returns. It checks the
proof and what the claim is for, then the recipient's whole history, then
that the claim was signed at a point of that history by a device the history
held then, for the incoming-share key it had then. The grant is wrapped to
the key the history has now, which a revocation since may have changed.

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
| Key agreement | x25519-dalek 3.0, without its precomputed tables, named only in `crypto.rs` | ring agrees with a key once and discards it, and a device's key is used for as long as the device lives. The one way to a key that is used twice in ring is a test helper it marks for removal |
| Points of small order | Refused as a one-time key and as a key to wrap to | Such a point agrees with every private key on 32 zero bytes, so what is wrapped under one is anyone's to open. Go and both Web Crypto libraries refuse it; x25519-dalek leaves it to the caller, who asks |
| Randomness and time | Handed in | The plan keeps the rules apart from random generation, and a case can then say which bytes must come out |
| Payload bytes | What Go's `encoding/json` writes for its struct: the fields in its order, `inbox` first when there is one, and its escapes | Nothing verifies by spelling. Two clients that write one statement two ways could not be held to one case |
| Private keys | Two types that print their public half and clear their bytes when dropped | A key that reaches a log is lost |
| Device requests | Read by the hosted service's rule | See the ninth finding |
| Recovery | One function from the key a person typed to the two private keys, checked against the history | Go and the viewer have the steps inside the code that talks to the service. Here a client cannot skip one |
| Context strings | Inside the crate, behind `State` | A vault sealed for the wrong epoch, or a wrapper for the wrong account, is a mistake a caller can no longer make |

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
| `keys.json` | 198: 29 boxes; 36 wrappers and 11 keys to wrap to, the seven spellings of a point of small order among both; 9 signatures; 31 device requests, 12 of them recorded differences; 17 accounts opened by a device and 23 by a recovery key; 42 shares for named people, of which a sender grants 7, refuses 32, and 3 are recorded differences |

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

A case in `keys.json` gives the secrets it was made with. Rust makes each one
again and must write the same bytes: the box, the wrapper, the signature, the
vault and the recovery package. It also seals every envelope that opens again
from its key and nonce, Go's and Web Crypto's among them. Go and the viewer
choose their own nonces and one-time keys, so they open each case, and sign
again where a statement is what a client writes. An account that is opened
starts from a record that verifies; what the case is about is whether the
device's key, or the recovery key, then reaches the vault's contents.

27 of the 34 histories every implementation accepts are marked as spelt the
way a client spells them. For those, Rust and Go each write the state they
read back to the same bytes, and the vault likewise. The other seven are
spelt some other way on purpose.

A share's case is what a sender holds when a claim arrives: its own history,
the invitation and its secret, the claim with its proof, and the recipient's
history. Rust and Go decide whether to grant. Where they do, Rust makes the
grant again, byte for byte, and Go signs the same statement around the
wrapper the case made. The viewer is the other side: it has no sender's rule,
and it opens each granted share as its recipient and every share as its
sender, through the code that verifies the invitation, the claim, the grant
and both histories before it unwraps anything. So a grant made the way Rust
makes one is a grant the viewer takes.

Go and the viewer keep three rules inside the code that talks to the service:
what a recovery key must match before it is used, what is checked of a
pending request, and Go's check of a claim. Their tests repeat those steps
with their own primitives. Rust has each as one function.

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

Since 2026-10-13 it does the same to a box, a wrapper and a device request:
every byte, every length and every character; the key and the context; a
private key with each of the bits that clamping ignores; a request's fields
each way, as a state's are. For a wrapper it also tries 63 numbers as the
one-time key, each with and without its unused top bit: the first 26, the 22
around the field's size, the two points of order eight, the largest, and 12
at random. Each is sealed twice, under what the recipient would agree on with
it and under 32 zero bytes.

On 2026-10-13 it ran 25,078 cases in 40 seconds. JavaScript, Go and Rust
judged 22,880 alike, and where all three accepted they read the same thing:
the same state at the same head, the same key, the same request. The other
2,198 are the kinds of recorded difference below. Anything else fails the
run, as does any case Rust accepts and another implementation refuses. All
three accept some cases of every kind, which the run now checks: 8 of 282
boxes, 121 of 707 wrappers, 62 of 586 requests. The `protocol` job in
`rust.yml` runs it on Linux, where the run at commit `46fd4a5` gave the counts
of the first slice: 23,503 cases, 21,521 alike.

With `--server` and a checkout of the hosted service it runs that verifier as
a fourth, on statements and histories. The service refused nothing Rust
accepted, and accepted 1,200 that Rust refused, all of them among the kinds
below.

[check-protocol-browser.mjs](../scripts/check-protocol-browser.mjs) runs the
case files through the viewer's code in a browser, because Node's Web Crypto
is OpenSSL and a browser brings its own. Headless Chromium 149 gave all 504
cases that are the viewer's to judge the verdict recorded for it.

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
   on it. It also refuses a whole identity of more than 10 MiB, which no
   client checks: the crate takes 512 events at their limit and a vault at
   its own, about 24 MiB, every signature checked. The client reads no more
   than 12 MiB of an answer. Found in the review of 2026-10-15, with the
   invitation secret in `Offer`'s debug output, which is now left out
   ([rust-client.md](rust-client.md#the-review-of-2026-10-15)).
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

The second slice found five more.

9. **Only the service says what a device request is.** Go and the viewer read
   a pending request, check that the key it names signed it, and compare its
   account, root key and time. Neither looks at the rest: a name of 81 bytes,
   a nonce that is not 24 bytes, an agreement key that is not a key, a
   request with no name at all. The service refuses each of those when the
   request is uploaded, so a client only ever sees them from a service that
   is not the real one. An approving client would then wrap the vault key and
   sign an event that every verifier refuses. Rust reads a request by the
   service's rule, and the sweep holds its refusals to a second statement of
   that rule: 144 cases, none refused that the service would store.
10. **Go and JavaScript write five characters differently.** Go's
    `encoding/json` escapes `<`, `>`, `&`, U+2028 and U+2029 in a payload and
    `JSON.stringify` does not, so a device named `<Phone & tablet>` is one
    payload from the Go CLI and another from the viewer. Both verify, because
    a payload is signed as bytes. Rust writes Go's, and a request case with
    such a name pins it. The first slice's generator had written every case
    with `JSON.stringify`, which no name in it could show.
11. **A key has spellings, and every library reads each the same.** A
    one-time key with its unused top bit set, or written past the field's
    size, is the same point to Go, OpenSSL and x25519-dalek: all three open
    the wrapper to the same key, in two cases and in more than 70 of the
    sweep's.
    The spelling is part of what the wrapping key is derived from, so it is
    another wrapper and not a second reading of one. A point of small order
    is refused by all three, and by BoringSSL in Chromium 149, with the box
    sealed under the zero bytes such a point would agree on.
12. **The browser check took an error that says nothing for success.**
    Chromium's Web Crypto refuses a point of small order with an error whose
    message is empty, and the check decided by the message. Until that was
    seen it reported Chromium as opening eight such wrappers and wrapping to
    seven such keys. The check now counts any error, and so does the sweep.
    No earlier case was affected: every refusal before these carried a
    message of the viewer's own.
13. **Go grants a claim that names no key.** Go reads an account with no
    incoming-share key as having the empty one. A claim signed before its
    account turned sharing on, naming the empty key, then matches the state
    it was signed at, and Go's sender wraps the share's key to whatever key
    the account has since taken. The claim still needs the invitation's
    secret and an approved device's signature, so this releases nothing to
    anyone the sender did not invite. The service refuses a claim from an
    account that does not share, so no such claim is stored. Rust reads no
    key as none and refuses.

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

A device request is read by the same reader, so the rows above hold for it
too. These are its own, from the ninth finding:

| Device request | JavaScript | Go | Rust |
| --- | --- | --- | --- |
| A name longer than 80 bytes, a nonce that is not 24 bytes, an agreement key or a root key that is not a key | verified | verified | refused |
| No name, or no time | verified | verified | refused |
| A time with a fraction, in quotes, or before 1970 | verified | refused, but for the last | refused |
| The signing key under a name in another case | refused | verified | refused |
| More than 4 KiB as text | verified | verified | refused |

A claim is read by the same reader as well, and the sender's side is Go's
and Rust's alone:

| Recipient's claim | Go | Rust |
| --- | --- | --- |
| A field's name in another case | granted | refused |
| A field given twice | granted | refused |
| Signed before the account shared, naming no key | granted | refused |

Boxes, wrappers, signatures, and accounts opened by a device or a recovery
key have no recorded difference: on the 125 cases and on the sweep's 989, the
three open the same things to the same bytes. Go's approval code compares
letters by Unicode's upper case and Rust by ASCII's, which no case can show:
a code is twelve characters of base32.

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

[made.rs](../crates/slink-protocol/tests/made.rs) does the same for what the
crate makes, with keys chosen at random and the same on every run.

- 1,500 boxes, 600 wrappers, 300 envelopes, 200 statements, 500 requests and
  200 recovery identities open again to what went in, and read back as JSON.
- A box or a wrapper with one character changed, removed or added does not
  open: 3,900 of them. Nor does either under another key or in another
  context.
- A wrapper does not open for the one-time key it was made with, and a
  device's wrapper does not open for the recovery identity or at the next
  epoch.
- Of 4,000 damaged and correctly signed requests, each is refused or still
  names the key that signed it in a shape the service stores.
- 80 accounts are made with the crate alone, a first event by the recovery
  identity and a second that turns sharing on, and each history verifies as
  it grows. Between 40 pairs of them a share goes from invitation to claim to
  grant, and the recipient unwraps the key. The same claim with another
  secret, for another share, or against another history releases nothing.

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

The second slice, against `main` at `00334aa` built the same day:

| Build | Binary |
| --- | --- |
| Before the making slice | 8,454,400 bytes |
| With it, and a wrapper made and opened in `build-check` | 8,470,960 bytes |

Another 16 KiB. HKDF, HMAC and Ed25519 signing were linked for TLS; what is
new is X25519 on a key that is kept. The binary holds only what `build-check`
calls, a wrapper made and opened; the rest of the slice arrives with the
commands that call it. The lockfile holds 233 packages, from
228: x25519-dalek, curve25519-dalek and its derive macros, fiat-crypto and
rand_core. Without its precomputed tables x25519-dalek derives a public key
by the same ladder it agrees with, which costs a client nothing it would
notice: a handful of keys in a session.

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

- The recipient's and the service's side of a named share: reading an
  invitation and a grant, and the rules the service applies to all three
  statements. A sender needs neither. The address a share is for is text
  here; the rule for an email address is the client's.
- Claims in the sweep. The 42 cases are the only ones Go and Rust have
  judged together, and the viewer could not be a third.
- What a vault holds: the JSON under the vault key, with its receipts,
  incoming-share private keys and outbox. Here it is bytes. It belongs with
  the client, which is also where the next state is put together: this crate
  writes and signs the state it is given and does not decide what follows.
- Reading a record off the wire, where the service refuses a signed object
  with a field besides its payload and signature, and pinning a root and a
  head, which belong to the client.
- One function each in Go and the viewer for a recovery key and for a pending
  request, so that their tests call what the product runs. Both are left as
  they are; Go is replaced at P5.
- The limits on the accepting side run only in the sweep. `cargo test` shows
  that 513 events and an oversized statement are refused, not that 512 events
  and 32 devices are accepted.
- Firefox and Safari, whose Web Crypto is neither OpenSSL nor BoringSSL.
- The plan's independent review of the protocol port, which is a P3 line.
