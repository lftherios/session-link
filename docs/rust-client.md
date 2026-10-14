# Rust client: signing in, publishing and deleting

2026-10-14 · Implemented slice of the [Rust migration](rust-migration.md), the first part of P3 (backlog item 8) · `slink-rs` signs in, publishes a session as an encrypted link from the terminal and from the local viewer, sends a lost upload again as the same ciphertext, and takes a link offline. Against the hosted service's smoke server it signs in by email approval, and a browser opens what it published. Device keys, the vault and named shares are not ported yet. Run on macOS arm64 only; the six targets run it when the branch is pushed.

`crates/slink-client` is the hosted service as the CLI reaches it. It
composes the [protocol crate](rust-protocol.md), which seals the document,
with the [home](rust-cli.md#layout), which keeps the key. The commands and the
local server's endpoints that publish are in `crates/slink`. Go's behaviour is
the contract, from [cli](../go/internal/cli) and the commands in
[go/cmd/slink](../go/cmd/slink); where Rust departs from it is listed below.

## What works

| Surface | State |
| --- | --- |
| `login` | `--key rk_…` saves a pasted key. With no key, the browser flow: the service gives a code, the person confirms it on the configured server, and the key it grants is saved in `config.json`, mode 0600 |
| `logout` | Removes the key and the handle, as Go does; the account id stays |
| `push` | A capture by id or path, the newest one, or `--pick`. Validated, scanned for credentials, shown, confirmed (or `--yes`), sealed here and uploaded as ciphertext |
| `share` | The newest local session or the agent's newest for this project, whichever was active last, a session named by id or transcript, or `--pick`; imported first when it is a transcript. Signs in through the browser when there is no key and a person is at the terminal |
| `delete` | A link, `s/<id>` or a bare id; a bare id is tried as a pre-encryption session first, as Go tries it |
| Local viewer | `POST /api/publish-preview/<id>` and `/api/publish/<id>` publish a link; `/api/login/status`, `start` and `cancel` run the browser sign-in for a page; an excerpt's page has the publish control and says where it encrypts to; a published view keeps its link in `drafts/published/` |
| Not yet | Sharing with named people, `/api/identity` and `/api/named` answer 501 with one message. `/settings` and `/shared` load and show it |

## Layout

| File | What it holds |
| --- | --- |
| [config.rs](../crates/slink-client/src/config.rs) | `config.json` read and written as Go writes it, and the target a command uses: flag, then `SLINK_SERVER` or `SLINK_API_KEY`, then the saved sign-in, then `https://session.link` |
| [origin.rs](../crates/slink-client/src/origin.rs) | Which servers a key may be sent to |
| [transport.rs](../crates/slink-client/src/transport.rs) | The HTTP client, which follows no redirect; reading at most so many bytes of an answer; a failure in Go's words |
| [publish.rs](../crates/slink-client/src/publish.rs) | The upload: checks, the pending receipt, the request, the acknowledgement and the share's receipt |
| [login.rs](../crates/slink-client/src/login.rs), [delete.rs](../crates/slink-client/src/delete.rs) | Signing in by key or browser; deleting a share or a pre-encryption session |
| [publish.rs](../crates/slink/src/publish.rs), [account.rs](../crates/slink/src/account.rs) | The five commands, with Go's messages |
| [server.rs](../crates/slink/src/server.rs) | The local viewer's publish and sign-in endpoints |

## Choices

| Area | Choice | Reason |
| --- | --- | --- |
| Client | reqwest, async, on the runtime the viewer already has; a command makes one of its own | The local server publishes from inside its handlers, and reqwest's blocking client cannot run there |
| Redirects | None followed, for any request | A key sent to one server must not go on to another. An answer that points elsewhere is read as the answer, as Go's publishing client reads it |
| The link | Made on the configured server, whatever URL the answer holds | Go's rule: an answer cannot send the content key to another site's script |
| Pending receipt | Written whole and flushed before any byte is sent, in Go's shape, under `pending-shares/<sha256 of server and text>-<sha256 of envelope>.json` | The same session sent again to the same server finds it, checks that it still opens to the same text, and sends the same ciphertext, so the service answers with the same link |
| Vault backup | Reported as `setup` | No device keys exist in this build, and Go says `setup` on a machine that has none. The viewer then offers the settings page |
| Randomness | The content key and nonce from the operating system | The protocol crate takes them as arguments; the client is where they are made |
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
| Sharing with named people, recovery, devices | Work | Refused with 501 and nothing sent |
| A capture still being recorded | Its spool is assembled before publishing | Read as the file it is: the recorder, which writes spools, is not ported |

## What building it found

- **Go publishes to the wrong path for a server written with an empty query
  or fragment.** It is the configured host, so no key goes elsewhere, but the
  service answers with an error or a page. Left as it is in Go and recorded
  above.
- **Go's `delete` sends the API key over plain HTTP to any host.** Publishing
  holds a server to HTTPS or loopback; deleting did not. Rust holds both to
  the same rule. Left in Go.
- **A test that clears the environment still wrote the clipboard.** With no
  `PATH` at all, a program is still looked for in the system's own
  directories, so `pbcopy` was found and the test's link was copied. The CLI
  tests now name a `PATH` that does not exist. The differential runner already
  gave its commands an empty directory.

## Evidence

| Check | Result |
| --- | --- |
| [tests/service.rs](../crates/slink-client/tests/service.rs) against a stand-in service | A lost acknowledgement sent again as the same bytes and recovered as the link; a session that is not valid or holds a credential never sent, and no key made for it; no plaintext to an old server; servers refused before any request; no redirect followed by publishing or deleting; answers that do not acknowledge this envelope not taken as a link; a pending receipt that no longer opens to the text not sent; the browser sign-in polled to its grant and saved for the configured server; delete's requests and answers. Go's own tests of the same are among them |
| [tests/cli.rs](../crates/slink/tests/cli.rs) | Every refusal before a byte is sent, with Go's messages; a key saved and removed; `push` to a stand-in, the envelope opened with the printed key, the receipt kept, `delete` sent; the local viewer's publish, its 501 for named people and identity, and its refusals from another origin |
| `SLINK_BINARY=go/slink SLINK_CANDIDATE=target/release/slink-rs node scripts/compat.mjs` | 87 of 104, from 86. The `share` steps of the discovery case now agree |
| Server smoke, `SLINK_BINARY=target/release/slink-rs npm run smoke:encrypted` | Passes: the smoke's own `push`, decrypted with Web Crypto after the sender exits |
| The same with `SLINK_BROWSER_SCRIPT=`[check-publish-browser.mjs](../scripts/check-publish-browser.mjs) | Passes with `slink-rs` and with Go: `slink login` approved through email, `push` with the saved key, the link opened in headless Chromium with its wrong-key and missing-key cases, `delete`, then 410 |
| [check-viewer-browser.mjs](../scripts/check-viewer-browser.mjs), [check-security-browser.mjs](../scripts/check-security-browser.mjs) against `slink-rs` | Both pass |
| `cargo test` | 270 pass |

## Cost

The release binary on macOS arm64 is 8,652,912 bytes, from 8,470,960: 178
KiB. The lockfile gains the new crate and no other package, so all of it is
code already in the tree that now has callers: the build check built an HTTP
client but never sent a request over TLS.

## Not done

- Device keys: setup, approval, recovery, revocation and discarding an
  unfinished setup. Until then the vault is never written, and a link's key
  lives only in this machine's `shares/` and in the link.
- The vault's merge and retry on a conflict, and receipts listed by account.
- Named invitations, claims, grants, the outbox and its resume loop.
- The terminal's questions: `Publish? [y/N]`, `--pick`, and the sign-in
  `share` starts. They need a terminal, so no test answers them; the code that
  asks is a port of Go's line by line.
- The six targets. These tests have run on macOS arm64 only.
