import { chmod, mkdir, readdir, readFile, rename, rm, stat, writeFile } from "node:fs/promises";
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

export async function listCaptures() {
  let files = [];
  try {
    files = await readdir(CAPTURE_DIR);
  } catch {
    return [];
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
