# Rust client: signing in, publishing, deleting and device keys

2026-10-14, extended the same day · Implemented slices of the [Rust migration](rust-migration.md), the first three items of P3 (backlog item 8) · `slink-rs` signs in, publishes a session as an encrypted link from the terminal and from the local viewer, sends a lost upload again as the same ciphertext, and takes a link offline. It sets up recovery, asks for approval and gives it, recovers with the recovery key, revokes a device, and backs each new link up to the account's vault. Against the hosted service's smoke server it signs in by email approval, a browser opens what it published, and three machines share one account through the settings page. Named shares are not ported. The tests of both slices pass on all six targets in CI.

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
| Recovery and devices | `/settings` and its `POST /api/identity`, with Go's actions: `status`; `setup`, which shows the recovery key until `confirm-recovery`; `discard-setup`; `request` and `approve` by code; `recover` with the key; `revoke`; `sharing`, which adds the incoming-share key named shares will use; and `sync`. Errors carry Go's messages and statuses, and `setup_conflict` for the page's discard offer |
| Backup | A link `push`, `share` or a page publishes goes into the account's vault when this machine is an approved device. The answer says `synced`, `locked`, `setup` or `failed`, as Go's does, and the link and its receipt are kept whatever it says |
| Not yet | Sharing with named people: `/api/named` and a publish with recipients answer 501 with one message, and `/shared` loads and shows it |

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
| [vault.rs](../crates/slink-client/src/vault.rs) | What the vault holds and Go's rules for it; this machine's receipts merged in, and the vault's written out to `shares/` |
| [publish.rs](../crates/slink/src/publish.rs), [account.rs](../crates/slink/src/account.rs) | The five commands, with Go's messages |
| [server.rs](../crates/slink/src/server.rs) | The local viewer's publish, sign-in and identity endpoints |

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
| Named shares in the vault | Checked by Go's rules and written back with Go's fields | This build makes none, and must not drop or change another device's |
| Device file | Go's fields: the two keys in standard base64, the pins, the pending setup and recovery key | Nobody uses the software yet, so Rust need not read Go's files; the shape is kept so that the two stay easy to compare |
| Clipboard | `pbcopy`, `clip`, then `wl-copy` or `xclip`, as Go | The link is printed either way |

## Where Rust differs from Go

Each difference is a refusal or a request not made.

| Case | Go | Rust |
| --- | --- | --- |
| A server with an empty query or fragment, `https://host/?` or `https://host/#` | Accepted. The upload then goes to `/` on that host, with `/api/shares` as its query or dropped as a fragment; checked with Go's `net/url` | Refused as an invalid server |
| A server with a space, a backslash, a byte outside printable ASCII, or a host with characters other than letters, digits, `-`, `.`, `_` and `~` | Accepted where its URL parser accepts them | Refused, so that nothing in the text is read one way by the check and another by the HTTP client |
| A host that is only a port, `https://:443` | Accepted | Refused |
| `slink delete` against a plain-HTTP server that is not on this machine | Sends the API key | Refused with the publishing rule's message |
| Deleting a pre-encryption session | Its client follows redirects | Follows none |
| A transport failure | Go's error text | Go's words for a refused connection, a reset, a timeout and an unknown host; reqwest's text for anything else |
| The identity endpoint, `/api/identity` | Its own URL check, which takes `https://host/?` as Go's publishing check does and then asks `/` with `/api/identity` as the query | The publishing rule above |
| No API key | The request is sent with an empty key, and the service refuses it | 401 with `run slink login first`, and nothing sent |
| A pending approval request | Listed with its code | Listed without it. The code is what the person types from the other device, so the approving page is not given it; the settings page never showed it |
| A request dated more than a minute ahead | Listed | Not listed; the service does not keep one either |
| A link in the vault | Its URL parsed and compared by scheme, host and path | Compared as the text `<server>/s/<id>#key=<key>`, which is how every client writes it |
| A receipt with no account, from before receipts named one | Backed up when the service lists that ciphertext as the account's | Stays on this machine. Rust writes none, and the port reads no state Go wrote |
| A device file from an older Go client: a recovery key, no pinned root and no pending setup | Offered for discarding when the account's identity is another's | Refused with the pins' message. Rust never writes one |
| Sharing with named people | Works | Refused with 501 and nothing sent |
| A capture still being recorded | Its spool is assembled before publishing | Read as the file it is: the recorder, which writes spools, is not ported |

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

## Evidence

| Check | Result |
| --- | --- |
| [tests/service.rs](../crates/slink-client/tests/service.rs) against a stand-in service | A lost acknowledgement sent again as the same bytes and recovered as the link; a session that is not valid or holds a credential never sent, and no key made for it; no plaintext to an old server; servers refused before any request; no redirect followed by publishing or deleting; answers that do not acknowledge this envelope not taken as a link; a pending receipt that no longer opens to the text not sent; the browser sign-in polled to its grant and saved for the configured server; delete's requests and answers. Go's own tests of the same are among them |
| [tests/cli.rs](../crates/slink/tests/cli.rs) | Every refusal before a byte is sent, with Go's messages; a key saved and removed; `push` to a stand-in, the envelope opened with the printed key, the receipt kept, `delete` sent; the local viewer's publish, its 501 for named people, recovery set up through `/api/identity` and the next link backed up, and its refusals from another origin |
| [tests/identity.rs](../crates/slink-client/tests/identity.rs) against a stand-in that verifies each write with the protocol crate and takes it only against the head and revision it holds | Three machines on one account: setup, the recovery key shown until confirmed, links backed up and another account's left out, sharing turned on, approval refused with a wrong code and given with the right one, revocation with a new epoch, vault key and incoming-share key that the revoked device's old key does not open, recovery on a machine that never had a device, and no secret in what the service stored. Go's setup tests: an identity the service substituted refused from the setup on, with nothing backed up to it; an interrupted setup sent again unchanged; a setup lost to another device reported on every action until discarded, then joined by approval or recovery with fresh keys; only an unfinished setup discarded. Also an older vault and a replaced root refused with the pins unchanged, a backup that loses a race made again, and the lock between processes |
| The same journey with `SLINK_IDENTITY_BRIDGE` set to the server checkout's `scripts/identity-bridge.mjs` | Passes against the service's own identity store, which verifies each statement and applies its compare-and-swap |
| `SLINK_BINARY=go/slink SLINK_CANDIDATE=target/release/slink-rs node scripts/compat.mjs` | 87 of 104, from 86. The `share` steps of the discovery case now agree |
| Server smoke, `SLINK_BINARY=target/release/slink-rs npm run smoke:encrypted` | Passes: the smoke's own `push`, decrypted with Web Crypto after the sender exits |
| The same with `SLINK_BROWSER_SCRIPT=`[check-publish-browser.mjs](../scripts/check-publish-browser.mjs) | Passes with `slink-rs` and with Go: `slink login` approved through email, `push` with the saved key, the link opened in headless Chromium with its wrong-key and missing-key cases, `delete`, then 410 |
| The same with `SLINK_BROWSER_SCRIPT=`[check-identity-browser.mjs](../scripts/check-identity-browser.mjs) and `slink-rs` | Passes: a first share through email sign-in, recovery set up on the settings page, a second machine locked until approved by its code, a wrong code refused, revocation, recovery on a third machine that restores the exact link, and no recovery key, content key or text in any request to the service |
| [check-viewer-browser.mjs](../scripts/check-viewer-browser.mjs), [check-security-browser.mjs](../scripts/check-security-browser.mjs) against `slink-rs` | Both pass |
| `cargo test` | 289 pass |
| `rust.yml` at `579f589`, and again at `073ad8a` with device keys | Lint, the format, protocol and viewer jobs, and the build, tests and `build-check` on each of the six targets pass |

## Cost

The release binary on macOS arm64 is 8,652,912 bytes, from 8,470,960: 178
KiB. The lockfile gains the new crate and no other package, so all of it is
code already in the tree that now has callers: the build check built an HTTP
client but never sent a request over TLS.

Device keys take it to 8,884,496 bytes, another 226 KiB, with no new package.
It is the device actions, and the parts of the protocol crate that make
statements, which `build-check` did not call: the recovery package, requests,
events and vaults signed here.

## Not done

- Named invitations, claims, grants, the outbox and its resume loop. The
  vault's named shares are kept but not written to `named-shares/`, and none
  made here go into it, until then.
- The terminal's questions: `Publish? [y/N]`, `--pick`, and the sign-in
  `share` starts. They need a terminal, so no test answers them; the code that
  asks is a port of Go's line by line.
