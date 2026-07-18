import { appendFile, chmod, mkdir, readdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
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

/** Append pre-built objects (skeleton and/or spans) as JSONL, in order. */
export async function appendSpool(file, objs) {
  await mkdir(path.dirname(file), { recursive: true });
  await appendFile(spoolPath(file), objs.map((o) => JSON.stringify(o) + "\n").join(""));
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
  try {
    await stat(spool);
  } catch {
    return null;
  }
  const rl = readline.createInterface({
    input: createReadStream(spool, { encoding: "utf8" }),
    crlfDelay: Infinity,
  });

  const tmp = `${file}.${randomBytes(4).toString("hex")}.tmp`;
  const out = createWriteStream(tmp);
  const write = async (chunk) => {
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
  await rename(tmp, file);
  if (finalize) await rm(spool, { force: true });
  return spans;
}

/**
 * Finalize spools whose recorder died (tap crash, machine sleep past the
 * gap): anything idle longer than `olderThanMs` can't belong to a live
 * session, so it becomes a finished capture stamped with its last append
 * time. Fresh spools are left alone — they may belong to a concurrently
 * running `slink dev`.
 */
export async function recoverSpools(olderThanMs) {
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
    try {
      const s = await stat(spool);
      if (Date.now() - s.mtimeMs <= olderThanMs) continue;
      const file = spool.slice(0, -".spool".length);
      if ((await assembleSpool(file, { finalize: true, endedAt: new Date(s.mtimeMs).toISOString() })) != null)
        recovered += 1;
      else await rm(spool, { force: true }); // empty/corrupt spool — nothing to save
    } catch {
      /* skip; retried next start */
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

  // Sessions still spooling have no (or a stale) .json — snapshot-assemble
  // them so list/push/open keep seeing live recordings, as they always did.
  // Cheap when nothing changed: one stat pair per open session.
  for (const f of files) {
    if (!f.endsWith(".json.spool")) continue;
    const jsonName = f.slice(0, -".spool".length);
    const jsonFile = path.join(CAPTURE_DIR, jsonName);
    try {
      const [sp, js] = await Promise.all([
        stat(path.join(CAPTURE_DIR, f)),
        stat(jsonFile).catch(() => null),
      ]);
      if (!js || sp.mtimeMs > js.mtimeMs) {
        await assembleSpool(jsonFile);
        if (!files.includes(jsonName)) files.push(jsonName);
      }
    } catch {
      /* snapshot is best-effort; the finalizer will produce the real file */
    }
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
        in_progress: run.metadata?.in_progress === true,
      });
    } catch {
      /* skip unreadable file */
    }
  }
  // Filenames embed the timestamp, so name order is time order.
  out.sort((a, b) => (a.file < b.file ? 1 : -1));
  return out;
}
