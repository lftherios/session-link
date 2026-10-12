# session.link architecture review

2026-10-11 · implementation review and technical assessment · scope includes the CLI, viewer and hosted service.

The existing division of responsibility fits the product: capture and selection
stay local, publication creates an immutable artifact, and the hosted service
delivers encrypted material to a browser. Preserve that division. The main
architectural debt sits in persistence, operations, protocol maintenance and
the availability of the sender during named sharing. A framework or language
rewrite would leave most of these concerns intact.

The [one-page overview](architecture-overview.md) describes the system. The
[two alternatives](architecture-alternatives.md) compare a transactional
monolith with a stateless hosted platform. Findings below distinguish observed
implementation choices from their inferred consequences; they are not claims
of production incidents.

## Scope and evidence

| Surface | Implementation examined | Responsibility |
| --- | --- | --- |
| CLI and integrations | [commands](../go/cmd/slink), [importers](../go/internal/importers), [pi extension](../packages/pi-extension) | Discovery, source identity, capture, import, terminal actions and distribution |
| Local data and web service | [spool](../go/internal/spool), [handoff](../go/internal/handoff), [local server](../go/internal/open), [exporter](../go/internal/share) | Crash recovery, stable snapshots, drafts, local access and exact export scope |
| Format and viewer | [format](../packages/format), [viewer](../packages/viewer), [bundle build](../scripts/build-viewer.mjs) | Open schema, derived reading model, rendering, composition and decryption |
| Publication and identity | [encrypted upload](../go/internal/cli/encrypted.go), [envelope](../go/internal/sealed/sealed.go), [native identity](../go/internal/identity), [browser identity](../packages/viewer/browser-identity.ts) | Durable retry, keys, device history, vault, recovery and named grants |
| Hosted offering | [routes](../../session-link-server/app), [server libraries](../../session-link-server/lib), [Rust adapter](../../session-link-server/blobs/src/main.rs) | Accounts, authorization, storage, admission controls and browser delivery |
| Operation and delivery | [Fly configuration](../../session-link-server/fly.toml), [Dockerfile](../../session-link-server/Dockerfile), both CI workflows and [CLI packaging](../packaging/README.md) | Deployment topology, compatibility checks and released artifacts |

Checkout revisions were client `fb2da26956b44ec1705c8043129197a4ef52c544`
and server `2250b3ba4721314ad2c4e2b24177a5e177bdfb0c`, both initially clean.
Read-only Fly status showed release 58 with one running Amsterdam machine,
512 MB RAM, one shared CPU and a mounted 1 GB volume. The public health endpoint
returned `{"ok":true}`. This establishes topology and basic health, not that
every deployed byte matches these checkouts or that recovery works.

## Data ownership and trust boundaries

The source harness owns its history. Importers read it and retain available
provenance; an imported `session/v0` document is a reconstruction, not a claim
to have intercepted every event. The proxy records API traffic but cannot
capture a harness that ignores its configured endpoint. The pi extension
provides an additional in-process integration. Local previews freeze a source
snapshot, so continued agent work does not silently alter a prepared share.

The browser's local UI requests selection and publication from Go. Go constructs
the excerpt, validates and scans the finished export, saves pending ciphertext
and its key, then uploads. The host acknowledges a ciphertext hash; the CLI
checks it and constructs the link on the configured origin. This is a useful
separation between reading mutable local history and publishing an immutable
document. Open-world format preservation applies to full documents; the excerpt
export intentionally strips unselected fields and raw source payloads.

| State | Local device or browser | Hosted service |
| --- | --- | --- |
| Captures, source history, drafts and previews | Plaintext files protected by local permissions | Absent from the encrypted publishing path |
| Published document | Plaintext before encryption and after recipient decryption | Ciphertext, hashes, size, owner and timestamps |
| Bearer share key | Receipt, complete link and optionally encrypted vault | No plaintext key through the protocol |
| Named share key | Sender outbox and recipient after unwrapping its grant | Encrypted wrappers and access metadata, including invited email |
| Device and recovery material | Private device keys; user-held recovery secret | Public keys, signed history, wrappers and encrypted recovery package |
| Account authentication | CLI credential or browser session | Account identities, hashed API keys and authentication state |

The server's JavaScript supply chain is part of the confidentiality boundary:
an altered hosted viewer can use the same keys and plaintext as an honest one.
Non-extractable browser device keys do not prevent same-origin malicious code
from using them. The local viewer has a different distribution boundary because
its code ships inside the installed binary. Changing blob storage cannot remove
either boundary. The [identity contract](identity-encryption.md) also correctly
distinguishes email authentication, cryptographic device approval and recovery.

## Findings and priorities

### 1 Recovery and availability depend on one volume

**High priority.** The machine, JSON records and blob store form one failure
domain. A second machine cannot simply join: process-local locks, login codes,
rate limits and unreplicated storage would disagree. Fly volumes do not
automatically replicate between machines. [Fly volume documentation](https://docs.fly.io/volumes/overview)

The health check writes and removes a probe file. It does not read a stored
encrypted document, validate identity history, exercise authentication delivery
or prove restoration. A restore rehearsal should recover blobs, metadata,
tombstones, signed histories, encrypted vaults and service configuration
together. Restoring an older identity snapshot can trigger legitimate client
rollback rejection; resetting client pins is not a recovery strategy. Define
acceptable data loss and recovery time, then test them. Existing protocol docs
still list a restore rehearsal as open.

### 2 The blob adapter serializes unrelated reads and writes

**High priority before growth.** In [sealed-store.mjs](../../session-link-server/lib/sealed-store.mjs),
`getShare`, `saveShare` and tombstoning share one global store lock. Each blob
operation launches a Rust process and opens/closes FsStore. Each put additionally
walks and synchronizes the store tree in
[main.rs](../../session-link-server/blobs/src/main.rs). Two admitted downloads
therefore do not imply two simultaneous blob reads.

Upload deduplication and account listings enumerate metadata files;
`listOwnerShares` reads the entire share metadata directory before filtering.
Named listings similarly scan all named shares. Upload work can therefore grow
with total service data, even for a small account. The one-upload/two-download
limits and shared two-body admission gate bound some memory pressure but do not
remove this serialization or provide storage quotas.

iroh currently supplies local persistence and BLAKE3 addressing only. Its pinned
0.103.0 documentation explicitly calls that version unsuitable for production
quality use; there is no demonstrated requirement here for its peer transport.
Choose a supported storage path or explicitly justify retaining this version.
[iroh-blobs 0.103.0 documentation](https://docs.rs/iroh-blobs/0.103.0/iroh_blobs/)

### 3 Named access requires an available sender device

**High product priority.** The recipient can verify email, establish an approved
browser device and accept the invitation while the sender is offline, but still
cannot read it. [resumeNamed](../go/internal/open/named.go) polls every 15 seconds
only while the local viewer server runs; [NamedView](../packages/viewer/NamedView.tsx)
waits for the resulting grant. The always-on recording tap is a different process
and does not perform this work.

The dependency protects the binding between invitation secret and recipient
keys. Moving metadata to a database or adding a cloud job does not supply the
sender's cryptographic authority. Possible improvements are an explicitly
enabled local grant worker, another approved sender device, or a separately
designed trust mechanism for previously verified recipients. First contact
still needs an authorized key holder under the current protocol. Measure
acceptance-to-grant time and explain waiting states clearly.

### 4 The custom identity protocol has three verifiers

**High assurance priority.** Native Go, browser TypeScript and server JavaScript
implement signed history transitions and related checks. The design has valuable
protections: exact signed bytes, separated signature purposes, device/root pins,
revision checks, key rotation and atomic event-plus-vault updates. However,
primitive choices alone do not establish the security of the application
protocol. The implementation also has finite device-history and vault limits,
with no history compaction or recovery-root replacement.

Maintain a versioned protocol specification and a shared corpus of accepted
and rejected histories across all three implementations. Reuse pure verification
code between browser and server where practical, while retaining native
cryptography and explicit storage adapters. Commission the independent review
already identified in [identity-encryption.md](identity-encryption.md). Neither
proposed storage architecture removes this requirement.

### 5 Large documents still incur whole-document costs

**Medium priority, guided by measurements.** The local handoff can serve a smaller
reading copy before fetching recorded tool results, and the trace tree uses
windowed rendering. These help first display and DOM size. Hosted encrypted
shares still pass through a complete server buffer, a complete browser download,
AES-GCM decryption, JSON parsing, schema validation and indexing before display.
The 25 MiB plaintext cap does not bound peak memory to 25 MiB.

Excerpt sharing reduces the transmitted document, but it currently excludes
images and attachments. The format has attachment references without a complete
hosted attachment delivery system. Measure near-limit documents on modest phones
before raising limits. A worker can move parsing/indexing off the UI thread;
partial reads require a separately versioned authenticated chunk/manifest
format, not just HTTP range support. See [sealed-document.ts](../packages/viewer/sealed-document.ts)
and [handoff.go](../go/internal/handoff/handoff.go).

### 6 Legacy ingestion has weaker persistence guarantees

**High correctness priority while writes remain enabled.** `/api/runs` still
accepts plaintext uploads. Its [storage implementation](../../session-link-server/lib/storage.ts)
updates a shared dedupe JSON file without a lock, uses predictable `.tmp` paths,
and does not use the newer flushed replacement helper. Even one Node process
can interleave asynchronous uploads and lose dedupe updates or collide on the
temporary file. This is a code-identified concurrency risk, not an observed
production data-loss incident.

Preserve existing `/r/` readers, but choose explicitly whether legacy writes
remain supported. If so, move them onto the same transaction and durability
rules as encrypted shares. Do not describe the whole service as ciphertext-only:
that property applies to current clients' encrypted publishing paths.

### 7 Deletion and retention need a complete lifecycle

**High priority before sustained usage.** Tombstones deny future application
downloads while retaining blobs and iroh retention tags. A bearer-link retry
can revive its URL; a deleted named share cannot be revived. No physical garbage
collection, per-account storage quota or coordinated backup expiry is implemented
for encrypted shares. A free-space reserve eventually refuses writes rather
than reclaiming space.

Define logical deletion, any grace period, blob reclamation and backup expiry
together. Account for shared content references and interrupted uploads before
removing a blob. Existing recipients may retain keys and plaintext under any
architecture. [Encrypted sharing storage contract](../../session-link-server/docs/encrypted-sharing.md)

### 8 Recording is part of the agent request path

**Medium reliability priority.** Importing existing histories is isolated from
agent execution. The optional proxy is inline: it forwards requests and streams
responses while retaining capture data. [server.go](../go/internal/tap/server.go)
reads request bodies fully without an explicit size cap and retains roughly
32 MiB of response data per call. Concurrent calls multiply that memory, and a
stopped proxy remains unavailable to clients pointed at it.

The spool/recovery design protects stored captures, but it does not make the
proxy bypass itself on failure. Test disk-full and concurrent large-request
behavior separately from spool correctness. The tap's age-based automatic prune
runs at startup; captures, previews and draft revisions do not share one complete
retention policy. The recursive permission repair on each data command also
scales with all local files. Keep source import as the dependable default and
set explicit recorder and local-storage resource budgets.

### 9 Release compatibility and observability need stronger gates

**Medium priority.** A fresh viewer build matched the server's checked-in bundle
byte-for-byte during this review. Nevertheless, [build-viewer.mjs](../scripts/build-viewer.mjs)
copies to the other repository only when explicitly requested. The Go binary,
hosted bundle and published format package can advance independently. Generated
schema and secret-pattern parity is checked; cross-repository viewer provenance
and compatibility are not enforced by the inspected default CI workflows.

Client CI runs unit tests and Go tests without the server bridge. Server CI runs
API smoke tests, but does not configure the native client and real-browser smoke
scripts. Make one release gate exercise both repositories with fixed versions,
including excerpt omission, browser decryption, recovery, named grants and old
client compatibility. Record the viewer source revision and bundle digest in
release metadata.

The legacy ingest route has structured application logs; the encrypted routes
do not have equivalent outcome/latency instrumentation. Add content-free counts
for publication failures, busy responses, storage use, grant completion and
vault conflicts. These would distinguish storage pressure from protocol or UI
friction and provide evidence for selecting the larger architecture.

## Verification and limits

Checks completed for this review:

- Client and server `npm test -- --test-reporter=dot` passed.
- `npm run build:viewer`, viewer TypeScript checking and generated-artifact
  parity checking passed. Rebuilding caused no tracked implementation changes.
- `SLINK_IDENTITY_BRIDGE=…/session-link-server/scripts/identity-bridge.mjs go test ./...`
  passed, including the Go/Node identity integration; several unchanged packages
  used Go's test cache. The first sandboxed attempt was blocked from the build
  cache and loopback sockets; the authorized rerun passed.
- Rebuilt local and checked-in hosted viewer SHA-256 digests both began
  `eec340330565f26779368b73b2e226a2da`.
- Fly status and the public health endpoint confirmed the topology and basic
  health stated above.

These checks do not replace a production load test, browser/device compatibility
matrix, independent cryptographic review, or backup restoration. The production
volume's contents and customer sessions were not inspected. No code fixes,
deployment or data migration were performed as part of the review.
