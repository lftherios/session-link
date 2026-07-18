import { appendFile, chmod, mkdir, readdir, readFile, rename, rm, stat, utimes, writeFile } from "node:fs/promises";
import { createReadStream, createWriteStream } from "node:fs";
import { once } from "node:events";
import readline from "node:readline";
import { randomBytes } from "node:crypto";
import os from "node:os";
import path from "node:path";

/**
 * Local capture store — one session/v0 JSON per recording session, under
 * ~/.slink/runs (override with SLINK_HOME). Files are pure session/v0
 * documents so `push` and `npm run validate` operate on them directly;
 * the only capture-side state (in_progress) rides in run.metadata.
 */
const HOME = process.env.SLINK_HOME ?? path.join(os.homedir(), ".slink");
export const CAPTURE_DIR = path.join(HOME, "runs");
const CONFIG_FILE = path.join(HOME, "config.json");

/** ~/.slink/config.json — { api_key?, server? }, written by `login`. */
export async function readConfig() {
  try {
    return JSON.parse(await readFile(CONFIG_FILE, "utf8"));
  } catch {
    return {};
  }
}

export async function writeConfig(config) {
  await mkdir(path.dirname(CONFIG_FILE), { recursive: true });
  // 0600: the config holds the API key. chmod too — mode only applies on create.
  await writeFile(CONFIG_FILE, JSON.stringify(config, null, 2) + "\n", { mode: 0o600 });
  await chmod(CONFIG_FILE, 0o600);
  return CONFIG_FILE;
}

export function newCapturePath(now = new Date()) {
  const iso = now.toISOString();
  const stamp = `${iso.slice(0, 10).replaceAll("-", "")}-${iso.slice(11, 19).replaceAll(":", "")}`;
  return path.join(CAPTURE_DIR, `${stamp}-${randomBytes(3).toString("hex")}.json`);
}

/**
 * Atomic write — a crash mid-write must never corrupt a capture. The tmp
 * name is unique per write: two in-flight writes to the same capture must
 * never rename each other's tmp file out from under themselves (writers
 * should still serialize for ordering — see the proxy's write chain).
 */
export async function writeRun(file, run) {
  await mkdir(path.dirname(file), { recursive: true });
  const tmp = `${file}.${randomBytes(4).toString("hex")}.tmp`;
  await writeFile(tmp, JSON.stringify(run));
  await rename(tmp, file);
}

/* ------------------------------------------------------------- the spool */

/**
 * A recording session's append-only sidecar: line 1 is the run skeleton
 * (spans = [root]), every later line is one completed span. The recorder
 * appends and forgets — O(call) I/O and flat memory over arbitrarily long
 * sessions — and the pure session/v0 .json is assembled ONCE, at close.
 * (The old shape re-stringified and rewrote the whole run per call:
 * O(session²) cumulative I/O, and the day's traffic held in heap.)
 */
export function spoolPath(file) {
  return `${file}.spool`;
}

const pidPath = (file) => `${spoolPath(file)}.pid`;

/** JSON.stringify emits U+2028/U+2029 raw; line-based readers (readline
 *  included — verified) treat them as line breaks, which would tear a span
 *  across spool lines. Escaping them is still the same JSON value. */
const jsonLine = (o) =>
  JSON.stringify(o).replace(/\u2028/g, "\\u2028").replace(/\u2029/g, "\\u2029");

const lockPath = (file) => `${file}.lock`;

// Boot timestamp (ms, ± scheduler drift). Recorded in the owner sidecar so
// a spool from before a reboot reads as dead even when its pid has been
// recycled to some long-lived daemon — post-reboot pids are densely
// re-claimed, and a squatted pid would otherwise pin the spool live for
// the machine's whole uptime.
const bootMs = () => Math.round(Date.now() - os.uptime() * 1000);
const BOOT_TOLERANCE_MS = 15_000;

// Heartbeat horizon: recorders refresh their sidecar on every append and on
// a timer, so a fresh sidecar is the strongest liveness signal — it holds
// even when the boot token is wrong (wall-clock steps in VMs, NTP jumps).
const SIDECAR_FRESH_MS = 3 * 60 * 1000;

/** Thrown (finalize mode only) when a commit ABORTS to protect data — the
 *  spool grew under us, or the lock was lost. Distinct from returning null
 *  ("nothing there / nothing salvageable"): recovery callers must retry
 *  later, never rename the spool aside as corrupt — that misreading once
 *  destroyed the very spans the abort existed to preserve. */
export class CaptureCommitRetry extends Error {
  constructor(why) {
    super(`capture commit aborted: ${why} — will retry on a later pass`);
    this.name = "CaptureCommitRetry";
  }
}

/** Give up ownership of a spool: after this, any process's recovery pass
 *  may finalize it. A long-lived tap whose finalize keeps aborting must
 *  call this — its own live pid would otherwise pin the spool as
 *  live-owned (and the capture as "(recording)") for the daemon's whole
 *  lifetime, fencing out the very "later pass" the abort promised. */
export async function releaseSpoolOwnership(file) {
  await rm(pidPath(file), { force: true });
}

/** Atomic — a torn sidecar read once judged a live owner dead. Exported so
 *  recorders can heartbeat it on a timer. */
export async function touchOwnerSidecar(file) {
  const p = pidPath(file);
  const tmp = `${p}.${randomBytes(4).toString("hex")}.tmp`;
  await writeFile(tmp, JSON.stringify({ pid: process.pid, boot: bootMs() }));
  await rename(tmp, p);
}

/**
 * Is the recorder that owns this spool still running? (Exported for tests.)
 *
 * alive := pidAlive && (sidecarFresh || (bootMatches && pid !== 1))
 *
 * - sidecarFresh: heartbeats survive wall-clock STEPS (VM resume, NTP) that
 *   break the boot token — round-3 verified bare-metal sleep is fine but
 *   stepped clocks read live recorders as dead.
 * - bootMatches (non-heartbeat path): covers a recorder suspended past the
 *   heartbeat horizon (Ctrl-Z'd `slink dev`) on a same-boot machine.
 * - pid 1 never rides the boot branch: kill(1,0) is EPERM-alive forever,
 *   and a containerized recorder (default PID 1) killed with its container
 *   would otherwise pin the spool until host reboot.
 * - legacy bare-pid sidecars (pre-boot-token) keep plain pid semantics for
 *   the transition; they age out with their spools.
 */
export async function spoolOwnerAlive(file) {
  let pid;
  let boot = null;
  let sidecarMtime = 0;
  try {
    const p = pidPath(file);
    const raw = (await readFile(p, "utf8")).trim();
    sidecarMtime = (await stat(p)).mtimeMs;
    if (raw.startsWith("{")) ({ pid, boot = null } = JSON.parse(raw));
    else pid = Number(raw); // sidecar from before the boot token existed
  } catch {
    return false;
  }
  if (!Number.isInteger(pid) || pid <= 0) return false;
  let pidAlive;
  try {
    process.kill(pid, 0);
    pidAlive = true;
  } catch (e) {
    // EPERM means the process EXISTS but belongs to another user — alive.
    pidAlive = e?.code === "EPERM";
  }
  if (!pidAlive) return false;
  if (boot == null) return true; // legacy sidecar: plain pid semantics
  const fresh = Date.now() - sidecarMtime <= SIDECAR_FRESH_MS;
  const bootMatches = Math.abs(boot - bootMs()) <= BOOT_TOLERANCE_MS;
  return fresh || (bootMatches && pid !== 1);
}

/**
 * A tiny cross-process mutex for the commit step. Check-then-rename is
 * inherently non-atomic: a snapshot racing a finalizer through the same
 * μs-wide gap could revert a finalized capture to in_progress with no
 * spool left to repair from. The lock is held for the few milliseconds of
 * [re-check → rename → cleanup]; a crashed holder's lock breaks by age.
 */
const LOCK_BREAK_MS = 30_000; // locked section is ms; 30s tolerates a Ctrl-Z'd holder

async function withCommitLock(file, fn, { waitMs = 2500, onTimeout = "null" } = {}) {
  const lock = lockPath(file);
  const token = `${process.pid}:${randomBytes(6).toString("hex")}`;
  const deadline = Date.now() + waitMs;
  for (;;) {
    try {
      await writeFile(lock, token, { flag: "wx" });
    } catch (e) {
      if (e?.code !== "EEXIST") throw e;
      const age = await stat(lock).then((s) => Date.now() - s.mtimeMs, () => 0);
      if (age > LOCK_BREAK_MS) {
        // Break by RENAME: atomic, so exactly one waiter wins the right to
        // clear a crashed holder's lock — a bare stat-then-rm let two
        // waiters both "break" and both enter the commit section.
        const grave = `${lock}.${process.pid}.${randomBytes(3).toString("hex")}.stale`;
        await rename(lock, grave).then(() => rm(grave, { force: true }), () => {});
      } else if (Date.now() > deadline) {
        // A fresh-but-stuck lock: snapshots give up quietly (advisory), the
        // finalizer's caller reports it (the spool survives for recovery).
        if (onTimeout === "null") return null;
        throw new Error("capture commit lock is stuck");
      } else {
        await new Promise((r) => setTimeout(r, 50));
      }
      continue;
    }
    // stillHeld: a holder suspended past LOCK_BREAK_MS can be broken; before
    // the irreversible rename it must confirm the lock is still its own.
    const stillHeld = () => readFile(lock, "utf8").then((c) => c === token, () => false);
    try {
      return await fn(stillHeld);
    } finally {
      // Remove only our own lock — never a successor's after a break.
      if (await stillHeld()) await rm(lock, { force: true });
    }
  }
}

/**
 * Append spans as JSONL. Header-aware: whenever the spool doesn't exist or
 * is empty — first call, but also a spool recreated after a finalize the
 * recorder didn't see — the skeleton is laid down first, so a spool can
 * NEVER start with a bare span (assembly refuses those; see below). Each
 * append starts on a fresh line, so a torn earlier write self-terminates
 * into one droppable line instead of merging with the next span. A pid
 * sidecar marks the spool as owned by a live recorder.
 */
export async function appendSpool(file, spans, skeleton, isClosed) {
  await mkdir(path.dirname(file), { recursive: true });
  const spool = spoolPath(file);
  const size = await stat(spool).then((s) => s.size, () => 0);
  // The last gate before bytes land: a write wedged in the fs calls above
  // can resume AFTER the session finalized — checked here, µs from the
  // append, instead of only at chain-execution time. (If appendFile itself
  // races the finalize, the recreated spool has no skeleton and assembly's
  // schema gate refuses it — the capture survives; only this span is lost.)
  if (isClosed?.()) return;
  // Every append doubles as a heartbeat — freshness is the primary
  // liveness signal (see spoolOwnerAlive).
  await touchOwnerSidecar(file);
  const payload =
    (size > 0 ? "\n" : "") +
    (size === 0 ? jsonLine(skeleton) + "\n" : "") +
    spans.map((s) => jsonLine(s) + "\n").join("");
  await appendFile(spool, payload);
}

/**
 * Assemble a spool into its .json capture — streaming, so heap stays O(1)
 * regardless of session size. Span lines pass through verbatim after a
 * parse check (a crash mid-append leaves at most one partial trailing
 * line, which is dropped). Snapshot mode (finalize: false) keeps
 * in_progress and leaves the spool in place — how a live session stays
 * visible to list/push/open. Returns the span count, or null if no spool.
 */
export async function assembleSpool(file, { finalize = false, endedAt } = {}) {
  const spool = spoolPath(file);
  let spoolMtimeMs;
  let spoolSizeAtStart;
  try {
    const st = await stat(spool);
    spoolMtimeMs = st.mtimeMs;
    spoolSizeAtStart = st.size;
  } catch {
    return null;
  }
  // Snapshot mode is advisory and must never beat the authoritative
  // finalizer: remember what the .json looked like before assembling, and
  // bail inside the commit lock if either file changed hands (see below).
  const jsonBefore = await stat(file).then((s) => s.mtimeMs, () => null);
  const rl = readline.createInterface({
    input: createReadStream(spool, { encoding: "utf8" }),
    crlfDelay: Infinity,
  });

  const tmp = `${file}.${randomBytes(4).toString("hex")}.tmp`;
  const out = createWriteStream(tmp);
  // Persistent listener: a disk error (ENOSPC — likely exactly when big
  // spools assemble) surfacing while we await a readline chunk must fail
  // this assembly, not crash the whole process as an uncaughtException.
  let streamErr = null;
  out.on("error", (e) => {
    streamErr ??= e;
  });
  const write = async (chunk) => {
    if (streamErr) throw streamErr;
    if (!out.write(chunk)) await once(out, "drain");
  };

  let skeleton = null;
  let spans = 0;
  try {
    for await (const line of rl) {
      if (!line.trim()) continue;
      let obj;
      try {
        obj = JSON.parse(line);
      } catch {
        continue; // partial trailing line from a crash — drop it
      }
      if (!skeleton) {
        if (typeof obj?.schema !== "string") {
          // Not a run skeleton — a torn or headerless spool. Refuse: never
          // fabricate a document (early versions promoted a stray span to
          // skeleton and overwrote good captures with schema-less garbage).
          out.end();
          await once(out, "close");
          await rm(tmp, { force: true });
          return null;
        }
        skeleton = obj;
        const root = skeleton.spans?.[0] ?? { id: "root", parent_id: null, type: "agent" };
        if (finalize) {
          root.ended_at = endedAt ?? new Date().toISOString();
          root.status = "ok";
          if (skeleton.metadata) delete skeleton.metadata.in_progress;
        }
        const head = { ...skeleton };
        delete head.spans;
        const headJson = JSON.stringify(head);
        const glue = headJson === "{}" ? '"spans":[' : ',"spans":[';
        await write(headJson.slice(0, -1) + glue + JSON.stringify(root));
        continue;
      }
      spans += 1;
      await write("," + line.trim());
    }
    if (!skeleton || spans === 0) {
      // Drain cleanly before removing — destroying with an in-flight write
      // fires ERR_STREAM_DESTROYED out of band.
      out.end();
      await once(out, "close");
      await rm(tmp, { force: true });
      return null;
    }
    await write("]}");
    out.end();
    await once(out, "finish");
  } catch (e) {
    out.on("error", () => {}); // teardown races are expected here
    out.destroy();
    await rm(tmp, { force: true });
    throw e;
  }
  // The commit — serialized across processes: a finalizer and a snapshot
  // must never interleave their [re-check → rename → cleanup] sections.
  if (streamErr) {
    await rm(tmp, { force: true }); // a late stream error must not strand the tmp
    throw streamErr;
  }
  return withCommitLock(
    file,
    async (stillHeld) => {
      if (!finalize) {
        // The finalizer may have won while we streamed: if the spool is gone
        // or the .json changed under us, our snapshot is stale — discard it
        // rather than revert a finalized capture to in_progress
        // (unrepairable: the spool it needs no longer exists).
        const [spoolStill, jsonNow] = await Promise.all([
          stat(spool).then(() => true, () => false),
          stat(file).then((s) => s.mtimeMs, () => null),
        ]);
        if (!spoolStill || jsonNow !== jsonBefore || !(await stillHeld())) {
          await rm(tmp, { force: true });
          return null;
        }
        await rename(tmp, file);
        // Stamp the snapshot with the spool's own mtime: the freshness gate
        // (spool newer than json → re-assemble) must keep firing for appends
        // that landed while we streamed, and go quiet only when caught up.
        await utimes(file, new Date(), new Date(spoolMtimeMs)).catch(() => {});
        return spans;
      }
      // Finalize: if the spool GREW since our read pass — a recorder we
      // misjudged as dead, or appends racing a shutdown — destroying it
      // would delete the newer spans. Abort loudly (CaptureCommitRetry, not
      // null): everything stays on disk for the next pass.
      const nowSize = await stat(spool).then((s) => s.size, () => null);
      if (nowSize !== spoolSizeAtStart) {
        await rm(tmp, { force: true });
        throw new CaptureCommitRetry("spool changed during assembly");
      }
      if (!(await stillHeld())) {
        await rm(tmp, { force: true });
        throw new CaptureCommitRetry("commit lock was broken");
      }
      await rename(tmp, file);
      await rm(spool, { force: true });
      await rm(pidPath(file), { force: true });
      return spans;
    },
    // The finalizer must outwait a crashed holder's 30s break age; a
    // snapshot is advisory and gives up quietly.
    finalize ? { waitMs: 45_000, onTimeout: "throw" } : { waitMs: 2500, onTimeout: "null" },
  );
}

/**
 * Finalize spools whose recorder is DEAD — judged by pid liveness, not
 * mtime: an idle spool may belong to a live `slink dev` whose user is just
 * thinking (an early mtime-based version finalized those, and the live
 * recorder's next append then fought its own finalized capture). A dead
 * spool becomes a finished capture stamped with its last append time.
 * Called at tap startup and opportunistically from listCaptures, so
 * crashed `slink dev` sessions heal on the next list/push/open too.
 */
export async function recoverSpools() {
  let files = [];
  try {
    files = await readdir(CAPTURE_DIR);
  } catch {
    return 0;
  }
  let recovered = 0;
  for (const f of files) {
    if (!f.endsWith(".json.spool")) continue;
    const spool = path.join(CAPTURE_DIR, f);
    const file = spool.slice(0, -".spool".length);
    try {
      if (await spoolOwnerAlive(file)) continue; // a live recorder owns it
      const s = await stat(spool);
      if ((await assembleSpool(file, { finalize: true, endedAt: new Date(s.mtimeMs).toISOString() })) != null)
        recovered += 1;
      else {
        // Unsalvageable as-is (empty, or a torn header ahead of durable
        // spans). Set it aside instead of deleting — the bytes may still
        // matter to someone, and a rename stops the rescan loop.
        await rename(spool, `${spool}.corrupt`).catch(() => {});
        await rm(`${spool}.pid`, { force: true });
      }
    } catch {
      /* skip; retried next start or next list */
    }
  }
  return recovered;
}

export async function listCaptures() {
  let files = [];
  try {
    files = await readdir(CAPTURE_DIR);
  } catch {
    return [];
  }

  // Spools: a live recorder's session gets a snapshot (so list/push/open
  // keep seeing live recordings, as they always did); a dead recorder's
  // gets finalized right here — a crashed `slink dev` heals on the next
  // list instead of waiting for a tap restart. Cheap when nothing changed:
  // one stat pair per open session.
  for (const f of files) {
    if (!f.endsWith(".json.spool")) continue;
    const jsonName = f.slice(0, -".spool".length);
    const jsonFile = path.join(CAPTURE_DIR, jsonName);
    try {
      const spoolFile = path.join(CAPTURE_DIR, f);
      const [sp, js] = await Promise.all([stat(spoolFile), stat(jsonFile).catch(() => null)]);
      if (await spoolOwnerAlive(jsonFile)) {
        if (!js || sp.mtimeMs > js.mtimeMs) await assembleSpool(jsonFile);
      } else if (
        (await assembleSpool(jsonFile, { finalize: true, endedAt: new Date(sp.mtimeMs).toISOString() })) == null
      ) {
        await rename(spoolFile, `${spoolFile}.corrupt`).catch(() => {});
        await rm(`${spoolFile}.pid`, { force: true });
      }
      if (!files.includes(jsonName)) files.push(jsonName);
    } catch {
      /* best-effort; the finalizer or the next pass will produce the file */
    }
  }

  // Orphan sidecars, locks, and lock graves: crashes at various points
  // leave each behind. Locks are removed via the same atomic rename-first
  // break as withCommitLock — a bare stat-then-rm could delete a
  // successor's fresh lock acquired in the gap.
  for (const f of files) {
    const orphanPid = f.endsWith(".json.spool.pid") && !files.includes(f.slice(0, -".pid".length));
    const orphanLock = f.endsWith(".json.lock");
    const orphanGrave = f.endsWith(".stale");
    if (!orphanPid && !orphanLock && !orphanGrave) continue;
    const horizon = orphanPid ? 3_600_000 : 10 * 60_000;
    const full = path.join(CAPTURE_DIR, f);
    stat(full)
      .then((s) => {
        if (Date.now() - s.mtimeMs <= horizon) return null;
        if (!orphanLock) return rm(full, { force: true });
        const grave = `${full}.${process.pid}.sweep.stale`;
        return rename(full, grave).then(() => rm(grave, { force: true }), () => {});
      })
      .catch(() => {});
  }

  const out = [];
  for (const f of files) {
    // Opportunistic sweep: a kill between write and rename strands a tmp
    // file; anything older than an hour is safe to clear (live writes are
    // milliseconds old).
    if (f.endsWith(".tmp")) {
      const full = path.join(CAPTURE_DIR, f);
      stat(full)
        .then((s) => (Date.now() - s.mtimeMs > 3_600_000 ? rm(full, { force: true }) : null))
        .catch(() => {});
      continue;
    }
    if (!f.endsWith(".json")) continue;
    const file = path.join(CAPTURE_DIR, f);
    try {
      const run = JSON.parse(await readFile(file, "utf8"));
      // Accept both the current name and the pre-rebrand run/v0.
      if (run?.schema !== "session/v0" && run?.schema !== "run/v0") continue;
      let inProgress = run.metadata?.in_progress === true;
      if (inProgress) {
        const spoolExists = await stat(spoolPath(file)).then(() => true, () => false);
        if (!spoolExists && !(await spoolOwnerAlive(file))) {
          // Stranded mid-flight — no spool to finalize from, no live owner
          // (a pre-recovery crash, or the blast radius of a lost race).
          // Heal in place so it stops reading as "(recording)" forever and
          // becomes prunable again. Inside the commit lock, with the state
          // RE-READ there: an unlocked heal based on this loop's stale read
          // once overwrote a concurrent finalizer's full capture.
          const healed = await withCommitLock(file, async (stillHeld) => {
            const cur = JSON.parse(await readFile(file, "utf8"));
            if (cur.metadata?.in_progress !== true) return true; // already fine
            const spoolNow = await stat(spoolPath(file)).then(() => true, () => false);
            if (spoolNow || !(await stillHeld())) return false;
            delete cur.metadata.in_progress;
            const root = cur.spans?.[0];
            if (root && !root.ended_at) {
              const last = cur.spans[cur.spans.length - 1];
              root.ended_at = last?.ended_at ?? root.started_at;
              root.status ??= "ok";
            }
            await writeRun(file, cur);
            return true;
          }).catch(() => false);
          if (healed) inProgress = false;
        }
      }
      out.push({
        file,
        name: run.name,
        created_at: run.created_at,
        spans: Array.isArray(run.spans) ? run.spans.length : 0,
        models: [
          ...new Set(
            (run.spans ?? [])
              .filter((s) => s.type === "llm_call")
              .map((s) => s.model?.id)
              .filter(Boolean),
          ),
        ],
        in_progress: inProgress,
      });
    } catch {
      /* skip unreadable file */
    }
  }
  // Filenames embed the timestamp, so name order is time order.
  out.sort((a, b) => (a.file < b.file ? 1 : -1));
  return out;
}
