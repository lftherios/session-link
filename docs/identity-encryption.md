# Identity and encrypted sharing

2026-09-12, updated 2026-10-10 · deployed: encrypted sharing live since 2026-09-12
(v29), named sharing and email sign-in since 2026-10-01 (v30). Independent
protocol review and a volume restore rehearsal remain open.

The initial-setup identity pinning fix described below is in the client source;
it is not part of the deployment status above.

The Go client scans and encrypts the prepared export locally. The hosted
`session-link-server` stores ciphertext in iroh-blobs FsStore and serves it over
HTTPS for browser decryption. Email link/code and GitHub login identify the same
stable account model. Recovery and approved devices protect an encrypted backup
of the owner's complete private share links. Named-recipient shares now use approved browser devices and encrypted account
grants; see the [named-sharing guide](named-recipient-sharing.md). Passkeys remain
future work.

## First share, without an existing account

1. Run `slink view`, choose material and select **Share this view**.
2. Review the included text, title and comment, then choose **Publish link**.
   The client saves that exact export locally.
3. If needed, choose **Sign in to continue**. A separate tab offers email or
   GitHub. Email creates an account on first use and offers a magic link or code.
4. Enter the confirmation code displayed in the local viewer. The native CLI
   receives an API credential and saves it privately. The original viewer keeps
   the excerpt, comment, title and position through sign-in, cancellation and retry.
5. Back in the viewer, choose **Publish encrypted link**. Only then does the
   client upload the encrypted export and show `/s/<id>#key=…`.
6. **Recovery and devices** is optional. Set it up to back up your share keys
   and restore them elsewhere. Viewing and saving local excerpts need no account.

`/r/` remains the older plaintext service; existing uploads there are not
retroactively encrypted. New clients never fall back to plaintext uploading.

## Keys and their responsibilities

| Material | Purpose | Where it lives |
| --- | --- | --- |
| API credential | Authorizes uploads, deletions and access to account metadata/ciphertext | Native config, mode 0600; hashed on server |
| Per-share key, 32 random bytes | AES-256-GCM encryption of one exact export | Bearer-link fragment or encrypted named grants; local receipt and encrypted vault |
| Device Ed25519 key pair | Signs approvals, revocations and vault updates | Private seed on that device; public key in signed history |
| Device X25519 key pair | Unwraps the current vault key for that device | Private key on that device; public key in signed history |
| Incoming-share X25519 key pair | Unwraps content keys addressed to an account | Public key in signed history; private keyring in encrypted version-2 vault |
| Vault key, 32 random bytes | AES-256-GCM encryption of the saved-link backup | Memory during use; encrypted separately to every approved device and recovery public key |
| Recovery key, 32 random secret bytes plus account/root fingerprint | Unlocks the encrypted recovery package | User's password manager or offline copy |
| Recovery Ed25519 key pair | Authorizes initial setup and recovery enrollment | Private seed inside the encrypted recovery package; public key pins account crypto identity |
| Recovery X25519 key pair | Unwraps the vault key after device loss, including after vault rotation | Private key in recovery package; public key lets devices wrap a new vault key |

No iroh transport key is involved in this integration: FsStore runs as a private
storage subprocess, and delivery uses HTTPS. The server's session-signing secret
and email provider key are service credentials, separate from content encryption.

Account identity is an internal ID, independent of email addresses, GitHub
handles and device keys. New accounts get a random one; accounts created before
email sign-in keep `gh_<GitHub numeric id>`. Provider identities use stable subjects. Linking
a login method requires proof of both accounts; matching email strings never
merge accounts automatically. Account login authorizes service access and does
not unlock the vault or approve a decryption device.

## Recovery and device approval

Open **Recovery and devices** in the local viewer. First setup creates the
recovery identity, approves this device and encrypts its saved private links.
Save the displayed recovery key, then choose **I saved my recovery key**. Until
that acknowledgment, the key stays in the private local device file so closing
the page or losing the setup response cannot strand it. The acknowledgment
removes that local recovery secret. Ordinary approved devices never retain the
recovery signing seed or recovery encryption private key.

Before the first setup upload, native and browser clients durably pin their own
generated recovery root and initial signed history head. A server response must
match that identity before it can unlock the vault or enable later backups.
An interrupted setup retains the exact signed, encrypted request alongside the
pending recovery token: retry resends it if the server did not store it, or
verifies the stored identity if only the acknowledgment was lost. Successful
verification removes the pending request; acknowledging the recovery key removes
the token. Pending recovery tokens from older clients are also checked against
the returned identity before acceptance.

On a new device, account login shows a locked vault. **Request device approval**
creates new device key pairs and displays a 12-character code. On an approved
device, enter that code under **Waiting for approval**. The client verifies the
request signature and code before wrapping the vault key to the requested
public key. The new device selects **Check approval** to restore the exact links.
Requests expire after ten minutes; expired requests can be recreated.

Alternatively, enter the recovery key locally. It verifies the account/root
fingerprint, decrypts the recovery package and current recovery wrapper, and
signs enrollment of fresh device keys. This works after all original devices
are lost. Account login is still required to fetch the encrypted backup.

Revocation removes another device, rotates the vault key and any enrolled incoming-share key, re-encrypts the vault
and replaces every surviving device/recovery wrapper atomically. Every
incoming-share key is introduced exactly once: verifiers reject a history that
reinstates a retired key, so an old private key held by a revoked device never
opens future grants. A removed
signing key cannot approve devices or write future vault versions. Already
received links and content keys still work; revocation cannot erase those copies.
Revoke account API credentials separately on the hosted account page.

## Protocol v1

The protocol uses Go's [X25519](https://pkg.go.dev/crypto/ecdh),
[Ed25519](https://pkg.go.dev/crypto/ed25519), AES-GCM and
[HKDF-SHA-256](https://www.rfc-editor.org/info/rfc5869/). The service checks Ed25519
signatures with [Node crypto](https://nodejs.org/api/crypto.html#cryptoverifyalgorithm-data-key-signature-callback).
This is an application protocol using those primitives, not an implementation
of HPKE, MLS or an iroh identity protocol. Independent protocol review is
still open.

A signed object is `{payload, signature}`, both canonical unpadded base64url.
The payload is the exact UTF-8 JSON bytes. Signatures cover
`"slink/" + purpose + "/v1" + NUL + payload_bytes`, with separate purposes
`event`, `vault` and `request`. Named sharing adds `named-invite`,
`recipient-claim` and `named-grant`. A browser recovering with the recovery key
also signs and checks a local-only `recovery-check` to confirm the root key it
unsealed. Verification never reserializes JSON. The SHA-256
of decoded event bytes identifies each history head.

Each event contains account ID, sequence, previous head, operation, signer,
fixed recovery public keys/package, the current recovery wrapper, the
incoming-share public key once one is enrolled, key epoch and the complete
approved device list with wrappers. The root signs genesis. An active device
signs a single approval, revocation or `sharing` enrollment of the first
incoming-share key; the recovery root signs a single recovery enrollment.
Existing device public keys cannot change in place. Both client and server
verify the full chain and reject unapproved signers and invalid transitions.
Native and browser clients pin the root, previous history head and highest vault revision
locally; old or conflicting histories are rejected.

Vault snapshots separately sign account ID, increasing revision, current history
head, key epoch, writer and ciphertext. An event and its new vault snapshot
commit together using expected previous head/revision. Ordinary vault updates
also use compare-and-swap; a concurrent share-key backup merges and is attempted
up to three times in all. Disk replacement is atomic and flushed on both client and server.
Native processes use an advisory file lock to protect local device state.

A key wrapper uses a fresh ephemeral X25519 key pair. HKDF has an empty salt
and the UTF-8 info string
`"slink/wrap/v1/" + account + "/" + root_public + "/" + epoch + "/" + ephemeral_public + "/" + recipient_public`.
Public keys are base64url. The derived 32-byte key encrypts the vault key with
AES-GCM, a fresh 12-byte nonce and that same info as additional authenticated
data. The wrapper is `ephemeral_public + "." + base64url(nonce || ciphertext || tag)`.

Vault AES-GCM additional data is
`"slink/vault/v1/" + account + "/" + root_public + "/" + epoch`.
Recovery package plaintext is `root_signing_seed || recovery_X25519_private_key`
(64 bytes); its AES-GCM additional data is
`"slink/recovery/v1/" + account + "/" + root_public`.
The user token is
`slr1.base64url(SHA256(account || NUL || root_public)).base64url(secret32)`.
Every encryption generates a fresh 12-byte nonce and uses a 16-byte tag.

Approval requests sign account/root, both device public keys, display name,
a 24-byte random nonce and creation time. The code is the first twelve base32
characters of SHA-256(payload bytes), grouped 4-4-4. An approved client computes
it independently and requires the code from the new device. A device first
encountering an existing account history uses trust on first use; a client
creating an identity pins its own generated root before upload. This does not
provide a public transparency log or independently verified human identity.

## Storage, scope and limits

- Spools, imported captures, config and device private keys are written at mode
  0600 in mode 0700 data directories. A recorded capture is assembled with the
  process umask inside its 0700 directory and set to 0600 by the next CLI
  command that works on local data. That startup step repairs permissions on
  existing local data without following symlinks. Device records are scoped
  by server and account under `~/.slink/identity`. Their containing directory is
  mode 0700. These files rely on local OS/disk protection; no OS keychain
  integration is included yet.
- Completed links live under `~/.slink/shares`. Pending uploads retain ciphertext
  and a key before transmission, allowing idempotent retry after a lost response.
- Vault entries contain complete private links and ciphertext hashes. A
  version-2 vault also holds the incoming-share private keys and the named-share
  outbox: content keys, invitations with recipient emails, and invitation
  secrets. It never holds session documents. New share keys back up automatically from approved devices. A failed
  backup leaves the published link and durable local receipt intact and reports
  that backup needs a retry.
- Receipts carry an account ID. Older unscoped receipts are included only when
  the authenticated account owns the exact share ID and ciphertext hash. Other
  account/operator-key receipts stay local. Old plaintext `/r/` uploads are not
  migrated.
- The server learns account IDs, public device/recovery keys, device names,
  approval/change history, pending requests, vault size and revision. It also
  sees share ownership, ciphertext sizes/hashes, upload/access times and normal
  HTTP metadata. It does not receive vault plaintext, private keys or recovery
  secrets through this protocol.
- Initial limits: 32 active devices, 512 device events, 16 pending requests,
  10,000 backed-up links and 4 MiB vault plaintext. Requests are capped at 12 MiB.
  Signed envelopes contain only payload and signature. Encoded payload limits
  are 4 KiB per device request, 32 KiB per event and 8 MiB per vault. The stored
  record plus pending requests is capped at 10 MiB, leaving response space for
  owned-share hints within the client's 12 MiB read limit. Body admission is
  shared across ingest, encrypted uploads and identity operations: at most two
  operations buffer data at once, including chunked requests.
  History compaction, recovery-key replacement, passkeys and
  key transparency need separate work.

The hosted recipient viewer is trusted JavaScript: compromised delivered code
could read its fragment key and decrypted session. A complete link also grants
access to anyone it is forwarded to. The native recovery/device UI is served
from the local CLI bundle. On an approved browser device the hosted bundle
serves the same **Recovery and devices** page at `/devices`: it holds that
browser's device keys, decrypts the vault with its backed-up links and
incoming-share keys, and accepts a typed recovery key, so compromised delivered
code could read those as well. Loss of all approved devices and the recovery key
makes old backed-up links unrecoverable through account login alone.

## Verification and deployment

Native and browser setup regression tests reject a valid server-substituted
identity both in the initial response and after a restart. They also exercise
retries before server commit and after a lost acknowledgment, checking that the
same recovery token and signed setup request survive. These tests run in the
ordinary Go and JavaScript suites without the server bridge.

Local checks cover Go/Web Crypto share interoperability, authenticated-encryption
tampering, wrong recovery keys, account-only lock, approval code mismatch, device
revocation and vault rotation, unauthorized/stale writes, root replacement,
rollback, exact-link recovery, and server storage privacy. Go/Node integration
exercises the actual signature/CAS store; it runs only with
`SLINK_IDENTITY_BRIDGE` set to the server's `scripts/identity-bridge.mjs`, so a
plain `go test ./...` skips it and most of the checks in this list with it. The real-browser smoke covers first
share through email account creation, cancel/retry, preserved excerpts, explicit
publication, and approval/recovery across three native device profiles.

See `docs/encrypted-sharing.md` in the server repository for the ciphertext
envelope, iroh version decision, Fly configuration, smoke commands and the
outstanding volume restore check. The single-machine Fly design runs in
production unchanged (as of v30, 2026-10-01) with Resend email sign-in
configured. These local checks perform no deployment or real email delivery.
