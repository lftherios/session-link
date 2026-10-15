# Rust client: signing in, publishing, deleting, device keys and named shares

2026-10-14, extended twice the same day, reviewed 2026-10-15 · Implemented slices of the [Rust migration](rust-migration.md), the first four items of P3 (backlog item 8) · `slink-rs` signs in, publishes a session as an encrypted link from the terminal and from the local viewer, sends a lost upload again as the same ciphertext, and takes a link offline. It sets up recovery, asks for approval and gives it, recovers with the recovery key, revokes a device, and backs each new link up to the account's vault. From the local viewer it shares an excerpt with named people and grants each their key once they accept. Against the hosted service's smoke server it signs in by email approval, a browser opens what it published, three machines share one account through the settings page, and a browser recipient accepts a named share and reads it. The tests of all three slices pass on all six targets in CI. A [review by four agents](#the-review-of-2026-10-15) found nothing that blocks a release; what it did find is fixed or recorded.

`crates/slink-client` is the hosted service as the CLI reaches it. It
composes the [protocol crate](rust-protocol.md), which seals the document and
makes and verifies every signed statement, with the [home](rust-cli.md#layout),
which keeps the keys. The commands and the local server's endpoints are in
`crates/slink`. Go's behaviour is the contract, from
[cli](../go/internal/cli), [identity](../go/internal/identity/client.go) and
the commands in [go/cmd/slink](../go/cmd/slink); where Rust departs from it is
listed below. [identity-encryption.md](identity-encryption.md) describes the
protocol the device keys follow.

## What works

| Surface | State |
| --- | --- |
| `login` | `--key rk_…` saves a pasted key. With no key, the browser flow: the service gives a code, the person confirms it on the configured server, and the key it grants is saved in `config.json`, mode 0600 |
| `logout` | Removes the key and the handle, as Go does; the account id stays |
| `push` | A capture by id or path, the newest one, or `--pick`. Validated, scanned for credentials, shown, confirmed (or `--yes`), sealed here and uploaded as ciphertext |
| `share` | The newest local session or the agent's newest for this project, whichever was active last, a session named by id or transcript, or `--pick`; imported first when it is a transcript. Signs in through the browser when there is no key and a person is at the terminal |
| `delete` | A link, `s/<id>` or a bare id; a bare id is tried as a pre-encryption session first, as Go tries it |
| Local viewer | `POST /api/publish-preview/<id>` and `/api/publish/<id>` publish a link; `/api/login/status`, `start` and `cancel` run the browser sign-in for a page; an excerpt's page has the publish control and says where it encrypts to; a published view keeps its link in `drafts/published/` |
| Recovery and devices | `/settings` and its `POST /api/identity`, with Go's actions: `status`; `setup`, which shows the recovery key until `confirm-recovery`; `discard-setup`; `request` and `approve` by code; `recover` with the key; `revoke`; `sharing`, which adds the incoming-share key named shares are wrapped to; and `sync`. Errors carry Go's messages and statuses, and `setup_conflict` for the page's discard offer |
| Backup | A link `push`, `share` or a page publishes goes into the account's vault when this machine is an approved device. The answer says `synced`, `locked`, `setup` or `failed`, as Go's does, and the link and its receipt are kept whatever it says |
| Named people | A page's publish with `recipients`: one to ten addresses, sharing turned on for the account if it is not, the excerpt sealed here and uploaded behind one signed invitation each, and each person's link, with the invitation's secret while they are still to accept. `/shared` and its `POST /api/named` list the shares with each person's status and revoke one person. While the viewer runs it looks every 15 seconds at up to eight unfinished shares and grants each person who has accepted, once their claim holds. Go's messages and statuses |

## Layout

| File | What it holds |
| --- | --- |
| [config.rs](../crates/slink-client/src/config.rs) | `config.json` read and written as Go writes it, and the target a command uses: flag, then `SLINK_SERVER` or `SLINK_API_KEY`, then the saved sign-in, then `https://session.link` |
| [origin.rs](../crates/slink-client/src/origin.rs) | Which servers a key may be sent to |
| [transport.rs](../crates/slink-client/src/transport.rs) | The HTTP client, which follows no redirect; reading at most so many bytes of an answer; a failure in Go's words |
| [publish.rs](../crates/slink-client/src/publish.rs) | The upload: checks, the pending receipt, the request, the acknowledgement and the share's receipt |
| [login.rs](../crates/slink-client/src/login.rs), [delete.rs](../crates/slink-client/src/delete.rs) | Signing in by key or browser; deleting a share or a pre-encryption session |
| [identity.rs](../crates/slink-client/src/identity.rs) | The device actions. Each reads the account's record, verifies it whole, checks it against this device's pins, and writes one event and its vault against the head and revision it read |
| [device.rs](../crates/slink-client/src/device.rs) | This machine's device for an account: its two keys and its pins in `identity/<hash of server and account>.json`, mode 0600, with the exact setup request until the service holds it |
| [vault.rs](../crates/slink-client/src/vault.rs) | What the vault holds and Go's rules for it; this machine's receipts and named shares merged in, and the vault's written out to `shares/` and `named-shares/` |
| [named.rs](../crates/slink-client/src/named.rs) | Named shares: the outbox, `named-shares/<hash of the ciphertext>.json` in Go's shape; publishing with invitations; listing, granting and revoking |
| [publish.rs](../crates/slink/src/publish.rs), [account.rs](../crates/slink/src/account.rs) | The five commands, with Go's messages |
| [server.rs](../crates/slink/src/server.rs) | The local viewer's publish, sign-in, identity and named endpoints, and the pass that goes on granting |

## Choices

| Area | Choice | Reason |
| --- | --- | --- |
| Client | reqwest, async, on the runtime the viewer already has; a command makes one of its own | The local server publishes from inside its handlers, and reqwest's blocking client cannot run there |
| Redirects | None followed, for any request | A key sent to one server must not go on to another. An answer that points elsewhere is read as the answer, as Go's publishing client reads it |
| The link | Made on the configured server, whatever URL the answer holds | Go's rule: an answer cannot send the content key to another site's script |
| Pending receipt | Written whole and flushed before any byte is sent, in Go's shape, under `pending-shares/<sha256 of server and text>-<sha256 of envelope>.json` | The same session sent again to the same server finds it, checks that it still opens to the same text, and sends the same ciphertext, so the service answers with the same link |
| Randomness | Every key, secret and nonce from the operating system | The protocol crate takes them as arguments; the client is where they are made |
| One action at a time | An async lock in the process, then an advisory lock on `identity/.lock` taken without waiting, as Go takes it, with the standard library's `File::try_lock` | Two processes must not pin from one answer and write from another; the service's compare-and-swap keeps other machines out |
| A backup that loses a race | Made again from the vault the service then holds, three times in all, for `sync` only | Go's rule. An approval or a revocation that loses is reported, and the person decides again |
| Setup | The identity is made, pinned and kept with the exact signed request before anything is sent | A retry sends the same request, so the recovery key shown is the one that works, and an answer that holds another root is refused from the first |
| Named shares in the vault | Read as Go reads them, checked by its rules, and written with its fields; this machine's merged in at each write, and the vault's written out where this machine has none | Another of the account's machines, or one recovered, goes on granting a share this one made |
| A share whose answer was lost | Its ciphertext, key and secrets kept in `named-shares/pending-<hash of server, account, text and people>.json` before upload, and sent again unchanged | The service takes the same ciphertext for the same people as the same share. A share it deleted answers 410, and the pending copy goes, so the next publish seals afresh |
| A claim's proof | Checked before the person's history is asked for, and the history asked for one person at a time | Go's order. A claim without the secret costs the service no second read, and ten people do not make one answer ten times as long |
| Granting while the viewer runs | A task on the viewer's runtime, first after 15 seconds, under the same two locks as every key action | Go's resume loop. The first pass after a restart is when a person who accepted meanwhile gets their key |
| Nothing shared yet | Listing with no `named-shares/` asks nothing and writes nothing | The viewer of someone who never shares leaves their home as it was. Go takes the lock first, so every Go viewer writes `identity/.lock` within 15 seconds of starting |
| Device file | Go's fields: the two keys in standard base64, the pins, the pending setup and recovery key | Nobody uses the software yet, so Rust need not read Go's files; the shape is kept so that the two stay easy to compare |
| Clipboard | `pbcopy`, `clip`, then `wl-copy` or `xclip`, as Go | The link is printed either way |

## Where Rust differs from Go

Each difference is a refusal or a request not made, save one: a claim that
does not hold no longer keeps the other people of its share waiting, and each
of their claims is checked in full as before.

| Case | Go | Rust |
| --- | --- | --- |
| A server with an empty query or fragment, `https://host/?` or `https://host/#` | Accepted. The upload then goes to `/` on that host, with `/api/shares` as its query or dropped as a fragment; checked with Go's `net/url` | Refused as an invalid server |
| A server with a space, a backslash, a byte outside printable ASCII, or a host with characters other than letters, digits, `-`, `.`, `_` and `~` | Accepted where its URL parser accepts them | Refused, so that nothing in the text is read one way by the check and another by the HTTP client |
| A host that is only a port, `https://:443` | Accepted | Refused |
| `slink delete` against a plain-HTTP server that is not on this machine | Sends the API key | Refused with the publishing rule's message |
| Deleting a share or a pre-encryption session | Its client follows redirects, and sends the key on to the same host or a subdomain | Follows none |
| A transport failure | Go's error text | Go's words for a refused connection, a reset, a timeout and an unknown host; reqwest's text for anything else |
| The identity endpoint, `/api/identity`, which named shares read first | Its own URL check, which takes `https://host/?` as Go's publishing check does and then asks `/` with `/api/identity` as the query | The publishing rule above |
| No API key | The request is sent with an empty key, and the service refuses it | 401 with `run slink login first`, and nothing sent |
| A pending approval request | Listed with its code | Listed without it. The code is what the person types from the other device, so the approving page is not given it; the settings page never showed it |
| A request dated more than a minute ahead | Listed | Not listed; the service does not keep one either |
| A link in the vault | Its URL parsed and compared by scheme, host and path | Compared as the text `<server>/s/<id>#key=<key>`, which is how every client writes it |
| A receipt with no account, from before receipts named one | Backed up when the service lists that ciphertext as the account's | Stays on this machine. Rust writes none, and the port reads no state Go wrote |
| A device file from an older Go client: a recovery key, no pinned root and no pending setup | Offered for discarding when the account's identity is another's | Refused with the pins' message. Rust never writes one |
| A capture still being recorded | Its spool is assembled before publishing | Read as the file it is: the recorder, which writes spools, is not ported |
| A saved key and a server named by `--server` or `SLINK_SERVER` that is not the one the key was issued for | Sends the saved key there | Sends none, as to a server never signed in to: the request goes without a key and that server decides |
| A write to the account's identity that the service answers as taken, then serves the record without | Pins only what it reads, so the older record passes and the backup or revocation is reported done | Pins what it wrote once the service has taken it, and refuses the older record |
| A field name in another case, `Recipients` in a publish request, `Shares` in the vault, `Key` in a sign-in grant | Read as the field | Refused. Every client writes the names exactly, and an exact reading would publish a named share as a link for anyone |
| A person revoked on this machine, or granted already, whom the service lists as accepted again | Granted, again | Not granted; the status this machine knows is kept |
| A share in which one person's claim does not hold | The pass over the share stops there, and the people listed after it get no key | That person is named in the share's error and passed over; the others are checked and granted |

## What building it found

- **Go publishes to the wrong path for a server written with an empty query
  or fragment.** It is the configured host, so no key goes elsewhere, but the
  service answers with an error or a page. Left as it is in Go and recorded
  above.
- **Go's `delete` sends the API key over plain HTTP to any host.** Publishing
  holds a server to HTTPS or loopback; deleting did not. Rust holds both to
  the same rule. Left in Go.
- **Go gives the approving page the code it asks the person for.** The
  status Go's client answers with lists each pending request with its
  confirmation code, the one the person is to read off the new device and
  type. The settings page does not show it, so the check holds, but a page
  that used it would approve a request the service had swapped. Rust leaves it
  out; Go is unchanged.
- **A test that clears the environment still wrote the clipboard.** With no
  `PATH` at all, a program is still looked for in the system's own
  directories, so `pbcopy` was found and the test's link was copied. The CLI
  tests now name a `PATH` that does not exist. The differential runner already
  gave its commands an empty directory.

## The review of 2026-10-15

Four agent reviewers, model sessions that had not seen the code, read the
protocol crate, device keys and the vault, named shares, and publishing. Each
was given the code, Go's code, the protocol documents and the threat model,
but not the author's conclusions. Each wrote tests for what it found; those
are now in the suites. None found a way to a key, a secret or the text, or a
check Go makes that Rust leaves out. They are not the independent review the
plan asks for before a stable release: they are the same kind of model that
wrote the code.

Fixed:

- **Requests to a server on this machine went through a proxy named in the
  environment.** reqwest honoured `HTTP_PROXY` and `ALL_PROXY` for
  `http://127.0.0.1`, so the API key crossed a proxy in the clear. Go never
  proxies this machine and reads neither variable for HTTPS. The client now
  takes `HTTPS_PROXY` for HTTPS, minus `NO_PROXY`, and nothing else
  ([tests/proxy.rs](../crates/slink-client/tests/proxy.rs)).
- **A write the service answered and dropped was reported done.** A backup
  said `synced`, and a revocation returned with the revoked device still in the
  history, still holding the vault key later backups would use. Go and the
  browser do the same. The device now pins what it wrote.
- **One claim that did not hold stopped every grant after it in its share.**
  The service cannot check a proof, so anyone holding an invited address
  could post one without the secret and hold up the rest. Go does the same.
- **The service's word replaced what this machine knew.** A person revoked here
  before their grant was granted when the service listed them as accepted
  again, and a person granted already was granted a second time, wrapped to
  whatever incoming-share key the service then showed. Go does the same.
- **Rust accepted some things Go refuses:** a server with a `%` that is not an
  escape, a sign-in grant whose fields are not text, a second copy of an
  approval request after one that fails, and field names in another case.
- **Smaller:** the invitation secret was in `Offer`'s debug output; the two
  locks were let go in the opposite order to Go's.

Recorded, not changed:

- **A sender takes the service's word for how current a recipient's history
  is.** A service working with whoever holds a device the recipient has since
  revoked can show the history as it was when they claimed, and the key is
  wrapped to the incoming-share key that device can open. Only an honest
  service's check stops it. It is the protocol's, shared by Go and the viewer,
  and stays with the broader protocol review the plan keeps; see
  [named-recipient-sharing.md](named-recipient-sharing.md).
- **A device revoked before its owner said the recovery key was saved still
  holds that key** in its device file, and can recover itself back in. A
  revoked device that asks again reuses its keys, though the design says a
  request makes new ones; a person still has to type its code. Both are Go's.
- **Finished and deleted named shares are never pruned.** A deleted one fails
  with 410 on every pass, and the outbox in the vault grows until, near 1,500
  ten-person shares, the vault passes its 4 MiB limit and every write to it
  fails, revoking a device among them.
- **A pending receipt left behind after its upload landed** sends the same
  ciphertext again, and the service revives a link that was deleted in
  between. Go and the service do the same.
- Keys are not wiped from memory when dropped. None reaches a log or an error.

## Evidence

| Check | Result |
| --- | --- |
| [tests/service.rs](../crates/slink-client/tests/service.rs) against a stand-in service | A lost acknowledgement sent again as the same bytes and recovered as the link; a session that is not valid or holds a credential never sent, and no key made for it; no plaintext to an old server; servers refused before any request; no redirect followed by publishing or deleting; answers that do not acknowledge this envelope not taken as a link; a pending receipt that no longer opens to the text not sent; the browser sign-in polled to its grant and saved for the configured server; delete's requests and answers. Go's own tests of the same are among them |
| [tests/cli.rs](../crates/slink/tests/cli.rs) | Every refusal before a byte is sent, with Go's messages; a key saved and removed; `push` to a stand-in, the envelope opened with the printed key, the receipt kept, `delete` sent; the local viewer's publish, recovery set up through `/api/identity` and the next link backed up, and its refusals from another origin; a publish for named people refused before recovery and for a bad address, then sent with sharing turned on, the excerpt opened with the outbox's key, each person's link, the listing and a revocation, a revocation the service refused not reported done, and no key or secret in any request |
| [tests/named.rs](../crates/slink-client/tests/named.rs) against a stand-in that holds each invitation and grant to the sender's current head and approved devices, as the service's named store does | Two people invited, each statement naming only its own; a person accepts, another machine recovered with the key has the share from the vault and grants it, and the person opens the text with their incoming-share key; the other still waits until revoked; no key, secret or text in a request. Go's tests: no grant for a proof made without the secret, another invitation's secret, a signature from elsewhere, a payload for another account or a device the history never approved, and no history asked for when the proof fails; a claimed invitation's link without its secret; a stale local status making no second grant; a person never invited stopping the pass after the grant before it is written down; a revocation with nothing kept here still sent; shares of a replaced identity and broken files left out of listings and the vault. Also a lost answer sent again as the same ciphertext and invitations, and a deleted share sealed afresh |
| [tests/identity.rs](../crates/slink-client/tests/identity.rs) against a stand-in that verifies each write with the protocol crate and takes it only against the head and revision it holds | Three machines on one account: setup, the recovery key shown until confirmed, links backed up and another account's left out, sharing turned on, approval refused with a wrong code and given with the right one, revocation with a new epoch, vault key and incoming-share key that the revoked device's old key does not open, recovery on a machine that never had a device, and no secret in what the service stored. Go's setup tests: an identity the service substituted refused from the setup on, with nothing backed up to it; an interrupted setup sent again unchanged; a setup lost to another device reported on every action until discarded, then joined by approval or recovery with fresh keys; only an unfinished setup discarded. Also an older vault and a replaced root refused with the pins unchanged, a backup that loses a race made again, and the lock between processes |
| The same journey with `SLINK_IDENTITY_BRIDGE` set to the server checkout's `scripts/identity-bridge.mjs` | Passes against the service's own identity store, which verifies each statement and applies its compare-and-swap |
| `SLINK_BINARY=go/slink SLINK_CANDIDATE=target/release/slink-rs node scripts/compat.mjs` | 87 of 104, from 86. The `share` steps of the discovery case now agree. Still 87 after the review's fixes |
| Server smoke, `SLINK_BINARY=target/release/slink-rs npm run smoke:encrypted` | Passes: the smoke's own `push`, decrypted with Web Crypto after the sender exits |
| The same with `SLINK_BROWSER_SCRIPT=`[check-publish-browser.mjs](../scripts/check-publish-browser.mjs) | Passes with `slink-rs` and with Go: `slink login` approved through email, `push` with the saved key, the link opened in headless Chromium with its wrong-key and missing-key cases, `delete`, then 410 |
| The same with `SLINK_BROWSER_SCRIPT=`[check-identity-browser.mjs](../scripts/check-identity-browser.mjs) and `slink-rs` | Passes: a first share through email sign-in, recovery set up on the settings page, a second machine locked until approved by its code, a wrong code refused, revocation, recovery on a third machine that restores the exact link, and no recovery key, content key or text in any request to the service |
| The same with `SLINK_BROWSER_SCRIPT=`[check-named-browser.mjs](../scripts/check-named-browser.mjs) and `slink-rs` | Passes: an excerpt shared with two people from the viewer, recovery set up there, the exact bytes behind the link, a browser recipient signed up by email who accepts, the sender's viewer restarted and granting on its own, a forwarded link refused to another account, the sender's browser approved from the native settings page, recovery and key rotation in the recipient's browser with the old grant still open, the person revoked from `/shared`, and no text, key or secret in a request or in the browser's storage |
| [check-viewer-browser.mjs](../scripts/check-viewer-browser.mjs), [check-security-browser.mjs](../scripts/check-security-browser.mjs) against `slink-rs` | Both pass |
| The five browser checks above, again with the review's fixes | All pass |
| `cargo test` | 311 pass, from 298: the reviewers' tests of a dropped backup and revocation, a bad claim among good ones, a revocation and a grant the service undoes, field names in another case, the proxy and the saved key's server |
| `rust.yml` at `579f589`, again at `073ad8a` with device keys, at `f99ee41` with named shares and at `d6c7ff6` with the review's fixes | Lint, the format, protocol and viewer jobs, and the build, tests and `build-check` on each of the six targets pass |

## Cost

The release binary on macOS arm64 is 8,652,912 bytes, from 8,470,960: 178
KiB. The lockfile gains the new crate and no other package, so all of it is
code already in the tree that now has callers: the build check built an HTTP
client but never sent a request over TLS.

Device keys take it to 8,884,496 bytes, another 226 KiB, with no new package.
It is the device actions, and the parts of the protocol crate that make
statements, which `build-check` did not call: the recovery package, requests,
events and vaults signed here.

Named shares take it to 9,016,800 bytes, another 129 KiB, again with no new
package: the outbox, the requests, and the protocol crate's invitations,
claim checks and grants, which nothing called before. The review's fixes add
16 KiB, to 9,033,360.

## Not done

- The terminal's questions: `Publish? [y/N]`, `--pick`, and the sign-in
  `share` starts. They need a terminal, so no test answers them; the code that
  asks is a port of Go's line by line.
