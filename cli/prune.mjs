import { rm, stat } from "node:fs/promises";
import path from "node:path";
import readline from "node:readline/promises";
import { listCaptures } from "./store.mjs";

/**
 * Local capture retention. The tap records ambiently, so ~/.slink/runs is a
 * rolling buffer, not an archive — and by design most captures are never
 * published. Published ones live on the server, so their local copy is
 * disposable; the rest are the "maybe I'll want this" buffer.
 *
 * A capture is pruned if it's empty (--empty), older than the age window, or
 * beyond the newest `keep` by position. In-progress captures — a session the
 * tap is still writing — are never touched.
 */

const DAY_MS = 86_400_000;

/** Pure planner: given captures (newest-first, from listCaptures) → what to remove. */
export function planPrune(captures, { now, olderThanMs = Infinity, keep = Infinity, empty = false }) {
  const remove = [];
  const kept = [];
  captures.forEach((c, i) => {
    if (c.in_progress) {
      kept.push(c); // never prune a live recording
      return;
    }
    const age = now - Date.parse(c.created_at ?? "");
    const isEmpty = (c.spans ?? 0) <= 1; // just the root agent span — nothing captured
    if ((empty && isEmpty) || (Number.isFinite(age) && age > olderThanMs) || i >= keep) remove.push(c);
    else kept.push(c);
  });
  return { remove, kept };
}

function fmtBytes(n) {
  return n >= 1e6 ? `${(n / 1e6).toFixed(1)} MB` : `${Math.max(1, Math.round(n / 1024))} KB`;
}

export async function pruneCommand(flags) {
  const captures = await listCaptures();
  // Bare `slink prune` defaults to a 30-day window; --keep/--empty on their own
  // don't impose an age cut unless --older-than is also given.
  const olderThanMs =
    flags["older-than"] != null
      ? Number(flags["older-than"]) * DAY_MS
      : flags.keep != null || flags.empty
        ? Infinity
        : 30 * DAY_MS;
  const keep = flags.keep != null ? Number(flags.keep) : Infinity;
  const { remove } = planPrune(captures, { now: Date.now(), olderThanMs, keep, empty: !!flags.empty });

  if (!remove.length) {
    console.error("nothing to prune");
    return;
  }
  let bytes = 0;
  for (const c of remove) {
    try {
      bytes += (await stat(c.file)).size;
    } catch {
      /* already gone */
    }
  }
  console.error(`${remove.length} capture${remove.length === 1 ? "" : "s"} to prune (${fmtBytes(bytes)}):`);
  for (const c of remove.slice(0, 8))
    console.error(`  ${(c.created_at ?? "").slice(0, 10) || "?"}  ${c.name ?? path.basename(c.file)}`);
  if (remove.length > 8) console.error(`  … and ${remove.length - 8} more`);

  if (flags["dry-run"]) {
    console.error("(dry run — nothing deleted)");
    return;
  }
  if (!flags.yes) {
    if (!process.stdin.isTTY) {
      console.error("not a TTY — pass --yes to prune from scripts");
      process.exit(1);
    }
    const rl = readline.createInterface({ input: process.stdin, output: process.stderr });
    const answer = (await rl.question(`Delete ${remove.length}? [y/N] `)).trim().toLowerCase();
    rl.close();
    if (answer !== "y" && answer !== "yes") {
      console.error("aborted — nothing deleted");
      return;
    }
  }
  let done = 0;
  for (const c of remove) {
    try {
      await rm(c.file, { force: true });
      done++;
    } catch {
      /* skip */
    }
  }
  console.error(`pruned ${done} capture${done === 1 ? "" : "s"} (${fmtBytes(bytes)})`);
}

/**
 * Silent retention sweep for the tap's startup: drop empty captures and
 * anything past the retention window (SLINK_RETAIN_DAYS, default 30; 0 to
 * disable). Best-effort — never throws. Returns how many were removed.
 */
export async function autoPrune() {
  const days = process.env.SLINK_RETAIN_DAYS != null ? Number(process.env.SLINK_RETAIN_DAYS) : 30;
  if (!(days > 0)) return 0;
  try {
    const captures = await listCaptures();
    const { remove } = planPrune(captures, { now: Date.now(), olderThanMs: days * DAY_MS, empty: true });
    let done = 0;
    for (const c of remove) {
      try {
        await rm(c.file, { force: true });
        done++;
      } catch {
        /* skip */
      }
    }
    return done;
  } catch {
    return 0;
  }
}
