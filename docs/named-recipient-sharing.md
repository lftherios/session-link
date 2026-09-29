# Named-recipient sharing

Recipients use a browser, including on their first visit. The browser is an
approved encryption device with private Web Crypto keys stored in IndexedDB.
Account login, device approval and recovery retain their existing boundaries.

A named share has a fresh ciphertext and an authenticated `/n/<id>` page. It is
never an existing bearer-link share with an added account check. The share key
stays out of the URL. Each recipient account gets a signed X25519 key wrapper.

Each account has an incoming-share X25519 key pair. Its public key is bound to
the signed device history; private keys stay in version-2 encrypted vaults.
Device revocation rotates both the vault key and incoming-share key; a retired
incoming-share key is never reinstated and verifiers reject reuse. The vault
retains older incoming-share private keys so remaining/new approved devices and
recovery can still open older grants. Old clients must reject the newer vault
version instead of dropping its fields during backup.

For a new recipient, the sender creates a private invitation for a verified email
identity and shares that link themselves. The invitation fragment contains a
random binding secret, never the session key. After browser sign-in and device
setup/unlock, the recipient signs its account/root binding and authenticates it
with that secret. The sender's native client verifies both proofs before issuing
the encrypted grant. The server alone cannot substitute a public key in that
handshake. Approved descendants of the bound history can receive the grant after
key rotation. First-time delivery therefore needs the sender's viewer running;
pending invitations persist locally and resume when the viewer is opened again.
The UI must say when it is waiting for acceptance or for the sender.

The invitation is restricted to the selected verified email. Forwarding the link
to another signed-in account does not authorize a claim or download. Once the
grant is ready, `/n/<id>` works without the invitation secret on approved devices.
Recipients can request approval from another device or use their recovery key
entirely in the browser. Clearing browser storage makes it a new device.

The server sees email/account access metadata and encrypted grants. Recipients
see only their own invitation. Browser code
is trusted, as with existing encrypted-link viewing. Revoking a recipient stops
new downloads; it cannot erase a key or plaintext already received.

Implementation checks:

- [x] Browser/native identity and key-wrapping interoperability; pinned history.
- [x] Version-2 vault migration and incoming-share key rotation on revocation.
- [x] Signed invitation, recipient binding and grant; no key in HTTP or link.
- [x] New-account browser acceptance, sender resume, exact selected export.
- [x] Forwarded link, wrong account, fabricated binding and stale key rejection.
- [x] Browser recovery/device approval and old-share access after rotation.
- [x] Owner-only recipient revocation and no bearer endpoint bypass.

Browser primitives follow [Web Cryptography Level 2](https://www.w3.org/TR/WebCryptoAPI/).
The protocol is an application of the existing X25519/HKDF/AES-GCM construction;
it does not claim compatibility with HPKE or MLS.

## Using it

In the native viewer, prepare an excerpt and choose **Publish link → Specific
people**. Enter up to ten email addresses. Complete recovery setup/device approval
if needed, then select **Share with these people**. Copy and send each invitation
link yourself; this implementation sends no invitation emails.

Recipients sign in with the invited email, or explicitly link that verified email
to their existing account. They set up recovery on their first browser, or unlock
an existing identity through device approval/recovery. **Accept invitation** binds
their encryption identity to this share. Keep the sender's local viewer running
until the grant is ready. This applies to each new share; an offline recipient
address directory and automatic grants to previously known people are future work.

**Shared with people** in the native viewer lists pending, granted, expired and
revoked access. The hosted account page lists incoming/outgoing shares and links
to browser **Recovery and devices**. An approved browser can open a ready share
from its canonical `/n/<id>` URL. A fresh login alone cannot decrypt it. The
invitation link, which carries the recipient's secret, is offered only while an
invitation is waiting; accepted and granted recipients get a link without it.

Unaccepted invitations expire in seven days. Revoke individual invitations from
the native list, or delete the whole share on the hosted account page. Revoking
one email invitation does not revoke a separate invitation to another verified
email on the same account. Named ciphertext tombstones are not revived by retry:
the API answers 410 and the native client discards its pending copy, so
publishing again creates fresh ciphertext. Already downloaded copies remain.

## Wire and persistence

`POST /api/named-shares` takes one signed invitation per recipient followed by
the encrypted envelope in a single body; `x-slink-invitations` gives the byte
length of the JSON invitation prefix. Each invitation binds the owner
account/root, current device history head/signer, ciphertext SHA-256, owner's
incoming key and wrapper, and exactly one recipient's email, random 24-byte ID
and SHA-256 commitment to its 32-byte secret. No invitation names any other
recipient, so a recipient never learns who else received the share, or how many
did. One invalid invitation rejects the whole upload before anything is stored.
Re-uploading the same ciphertext with the same recipient set is idempotent even
when re-signed at a newer head; a different set answers 409. The native client
saves the ciphertext, key and invitation secrets before upload. After acknowledgment it retains a private outbox under
`~/.slink/named-shares` and backs it up inside the encrypted vault.

Invitation URLs are `/n/<share>?invite=<id>#invite=<secret>&sender=<root>`.
The fragment authenticates the recipient binding and pins the sender root. It
is never the session key. The browser keeps it in tab-scoped session storage
through sign-in, then removes it and the invitation query once access opens.

Claims use signature purpose `recipient-claim`. Their exact JSON payload binds
recipient account/root, approved device/head, incoming public key/epoch, share ID,
invitation ID, ciphertext SHA-256 and owner account/root. The additional proof is
HMAC-SHA-256 with the invitation secret over
`"slink/recipient-binding/v1" || NUL || payload_bytes`.

Grants use purpose `named-grant`, binding owner account/root/head/signer,
share/invitation/hash, recipient account/root/incoming key/epoch, the claim's
SHA-256 and the wrapped content key. Invitations use `named-invite`. All use the
existing exact-byte Ed25519 signing format. Grant and owner wrappers use the
existing X25519/HKDF/AES-GCM construction with the context
`named/<ciphertext_sha256>/<recipient_account>/<recipient_root>/<epoch>`.

The server checks active signing devices and current recipient encryption epochs
under the same locks used for identity changes. Native clients verify recipient
history and the secret proof before releasing keys. Browsers verify their invitation,
claim, grant, historical signers and ciphertext hash before authenticated
decryption, followed by schema/invariant checks of the plaintext.

Browser device private keys are nonextractable Web Crypto keys in IndexedDB,
scoped to the origin and account. Web Locks serialize updates across tabs. The
browser pins the account root, previous head and highest vault revision. Recovery
setup temporarily saves its user token locally until acknowledgment. Incoming
keyring and outbox secrets are in the encrypted vault, never IndexedDB plaintext.
Browsers need Ed25519, X25519, IndexedDB and Web Locks; unavailable primitives
produce an explicit update-browser error. Clearing storage requires approval or
recovery again. The native client continues to use private OS-protected files.

Identity and named-share metadata reads allow 120 requests per account/minute;
write limits retain the existing 30/minute default. Background sender work rotates
through at most eight pending shares every fifteen seconds. Recipient histories
are fetched individually so ten invitations do not multiply one response's bound.
Grants share one sender log per share and record only its length at grant time,
so a share stores at most one copy of the sender history however many
recipients it has. Existing limits of 32 active devices, 512 events and 4 MiB
vault plaintext still
apply. Version-2 vaults retain historical incoming keys and up to 10,000 outbox
entries within that byte limit. Saved private shares that cannot be verified for
the current identity, for example after the account is set up again, stay on
disk but are left out of listings, background grants and vault backups.
Version-1 accounts explicitly enroll their first incoming key with a signed
`sharing` event; legacy clients reject version 2.

## Verification

The Go identity tests cover migration, recovery of incoming keys, rotation and
forged invitation/signature rejection. The server's named-sharing tests cover
verified-email access, signed claims/grants, stale recipient keys, owner-only
revocation, retries and deletion. To run the real browser flow against the server
and iroh store, build the viewer/CLI, then in the server checkout run:

```sh
SLINK_BINARY=/tmp/session-link-slink \
SLINK_BROWSER_SCRIPT=../session-link/scripts/check-named-browser.mjs \
npm run smoke:encrypted
```

This uses a loopback mailbox and isolated browser profiles. It checks the exact
prepared bytes, first-account acceptance, sender restart, forwarded-link denial,
native-to-browser approval, browser recovery, incoming-key rotation, old grants,
recipient revocation, mobile layout and plaintext/key absence in hosted requests
and storage. These are local checks; Fly deployment and real mail delivery are
still pending, as is independent protocol review.
