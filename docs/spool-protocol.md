# The capture spool protocol (v1, frozen 2026-07-18)

The contract between a slink **recorder** (tap daemon / `slink dev`) and
every other slink process (`list`, `push`, `open`, `prune`, recovery) for
in-progress captures. The Go implementation targets this document, not the
JS code; the JS implementation (`cli/store.mjs` @ `7caaf91`, 106 tests) is
the reference. This protocol survived six adversarial verification rounds
(18 → 13 → 10 → 2 → 1 → 0 findings); every rule below exists because its
absence was a demonstrated bug — the rationale comments in `store.mjs` are
part of the spec.

## Files

For a capture at `<dir>/<stamp>-<hex>.json` ("the json"):

| Path | Role |
| --- | --- |
| `<json>.spool` | append-only JSONL of the live session |
| `<json>.spool.pid` | owner sidecar: `{"pid": int, "boot": intMs}` |
| `<json>.lock` | commit mutex (exists only during a commit) |
| `<json>.spool.corrupt` | a refused spool, set aside (forensics, never read) |
| `<json>.lock.<pid>.<hex>.stale` | a broken lock's grave (transient) |
| `*.tmp` | atomic-write staging (unique names, swept ≥1h) |

Capture paths embed a timestamp + 3 random bytes; two sessions never share
a path.

## Spool format

- Line 1: the **skeleton** — a `session/v0` document whose `spans` array
  holds only the root span, `metadata.in_progress: true`. Assembly MUST
  refuse a first line without a string `.schema` (never promote a span to
  skeleton; never fabricate a document).
- Every later line: one completed span, serialized as single-line JSON
  with U+2028/U+2029 escaped as `\u2028`/`\u2029` (JSON.stringify-style
  emitters leave them raw; line-based readers split on them).
- Appends: written with O_APPEND; each append is prefixed with `\n` when
  the spool is non-empty, so a torn earlier write self-terminates into one
  droppable line. Blank lines are skipped by readers. An unparseable line
  is dropped (crash-torn); if line 1 is unparseable the whole spool is
  refused (see `.corrupt`).
- The recorder lays the skeleton lazily: whenever the spool is missing or
  empty at append time. A session that never records leaves no files.

## Owner sidecar and liveness

- Written atomically (tmp+rename) on every append and on a ≥60s heartbeat
  timer while the session is open. Content: the recorder's pid and its
  boot timestamp (`now − uptime`, ms).
- Liveness rule (checker side):
  `alive := pidAlive && (sidecarFresh || (bootMatches && pid ≠ 1))`
  - `pidAlive`: signal-0 succeeds, **or fails with EPERM** (the process
    exists under another uid — that is alive).
  - `sidecarFresh`: sidecar mtime within 3 min (survives wall-clock steps
    — VM resume, NTP — that break the boot token).
  - `bootMatches`: |sidecar boot − checker boot| ≤ 15 s (covers a
    suspended recorder, e.g. Ctrl-Z, that can't heartbeat).
  - pid 1 never rides the boot branch (EPERM-forever would pin a dead
    containerized recorder).
  - Legacy sidecars (bare integer, no boot): plain `pidAlive`.
- `releaseSpoolOwnership` = delete the sidecar. A recorder that cannot
  finalize (retries exhausted, or a non-retryable error) MUST release so
  other processes can take over.

## Assembly

One streaming pass, O(1) heap: emit the skeleton header (minus `spans`),
then the root span — stamped `ended_at`/`status: "ok"` and
`in_progress` removed in finalize mode — then each span line verbatim,
into a unique `.tmp`, then commit (below). Returns the span count; `null`
means "no spool, or nothing salvageable" (empty / refused header). The
output stream must carry a persistent error listener: a disk error during
assembly fails the operation, never the process.

- **Snapshot mode** (`finalize: false`): keeps `in_progress`, leaves the
  spool; after commit, sets the json's mtime to the spool's pre-assembly
  mtime (so "spool newer than json" keeps triggering re-assembly until
  caught up). Used by `list`/`push`/`open` to see live sessions.
- **Finalize mode**: consumes the spool + sidecar after the rename.

## Commit protocol

All commits (both modes, plus the stranded-heal below) run inside the
lock: create `<json>.lock` with O_EXCL, content `pid:randomToken`.

- Wait: poll ≥50ms. A lock older than **30s** is a crashed holder: break
  it by **renaming it to a unique grave, then deleting the grave** — the
  rename is atomic, so exactly one breaker wins (stat-then-delete let two
  waiters both break and both enter). Sweepers must break the same way.
- Budget: finalizers wait ≥45s (must outlast the break age) and then
  error; snapshots give up quietly (≈2.5s) with no effect.
- Holders verify `stillHeld` (lock content equals their token) immediately
  before the irreversible rename, and delete the lock only if still theirs.
- Snapshot commit re-checks under the lock: spool still exists AND json
  mtime unchanged since pre-assembly — else discard the tmp (a finalizer
  won; renaming anyway would revert a finished capture to `in_progress`
  with no spool left to repair it).
- Finalize commit re-checks under the lock: spool size unchanged since the
  read pass AND `stillHeld` — else **abort with a retryable error**
  (`CaptureCommitRetry`), leaving every file in place.

## Retry vs. corrupt — the two abort meanings

- `CaptureCommitRetry` (spool grew; lock lost): transient. Callers retry
  later; recovery passes skip the file. The tap retries in-process
  (5 × 2s) and on exhaustion releases ownership. **Never** set a
  retry-aborted spool aside as corrupt — the growth usually means a live
  recorder was misjudged dead.
- `null` with the spool present (refused header / empty): structural.
  Recovery renames the spool to `.spool.corrupt` (preserving bytes,
  stopping rescan loops) and deletes the sidecar.

## Recorder session rules

- Per-session serialized write chain; span ids `s<n>` assigned in order.
- `pending` counter: incremented before any await once a call is assigned
  to a session; sessions with `pending > 0` are never idle-finalized or
  rolled over (a stream can outlast any idle gap).
- `closed` flag: set at finalize entry. Checked (a) before enqueueing an
  append, (b) at chain-execution time, (c) inside append µs before the
  write. A span arriving after close is dropped with a warning. If the
  final race fires anyway, the recreated spool has no skeleton and
  assembly's schema gate refuses it: the finished capture survives; only
  the late span is lost.
- Shutdown: stop intake → drain in-flight (bounded) → finalize all.
  Drains are bounded (a wedged disk must not hold exit hostage).
- Root `ended_at` = last recorded span's `ended_at` (not wall-clock at
  finalize — wake-after-sleep would stamp the wrong time).

## Recovery passes

Run at tap startup and opportunistically during `list`:

- Dead-owner spools (liveness rule): finalize with
  `endedAt = spool mtime`; on structural refusal, `.corrupt`-aside.
- Live-owner spools: snapshot if the spool is newer than the json.
- Stranded `in_progress` json with no spool and no live owner: heal in
  place (drop the flag, stamp the root from the last span) — inside the
  commit lock, with the json re-read there.
- Sweeps: `.tmp` ≥1h; orphan `.spool.pid` (no spool) ≥1h; `.json.lock`
  ≥10min (via grave-rename); `.stale` graves ≥10min.

## Accepted residuals (documented, not bugs to fix)

- Same-boot pid recycling to a live non-1 process within the heartbeat
  horizon reads falsely alive until that process exits.
- A `slink dev` crash heals only when some slink command later runs.
- Ctrl-Z'd recorder on a machine whose clock also stepped >15s fails both
  liveness branches (suspension defeats heartbeats; the step defeats the
  boot token).
- An append wedged in the final write syscall across a shutdown can lose
  that one span (never the capture).
