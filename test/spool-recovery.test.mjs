import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readFile, rm, stat, utimes, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

// SLINK_HOME must bind before the store module loads CAPTURE_DIR.
const home = await mkdtemp(path.join(os.tmpdir(), "slink-recover-"));
process.env.SLINK_HOME = home;
const { CAPTURE_DIR, appendSpool, assembleSpool, listCaptures, spoolOwnerAlive, spoolPath } =
  await import("../cli/store.mjs");

const skeleton = (name = "r") => ({
  schema: "session/v0",
  name,
  created_at: "2026-07-18T10:00:00.000Z",
  source: { kind: "proxy", label: "t", fidelity: "exact" },
  metadata: { in_progress: true },
  spans: [{ id: "root", parent_id: null, type: "agent", name, started_at: "2026-07-18T10:00:00.000Z" }],
});
const span = (i) => ({
  id: `s${i}`, parent_id: "root", type: "llm_call", model: { id: "m" },
  input: { messages: [] }, output: { messages: [] },
  started_at: "2026-07-18T10:00:01.000Z", ended_at: "2026-07-18T10:00:02.000Z",
});
const cap = (n) => path.join(CAPTURE_DIR, `${n}.json`);

test("owner liveness: own live pid is alive; stale boot token reads dead", async () => {
  const f = cap("20260718-100000-aaaaaa");
  await appendSpool(f, [span(1)], skeleton());
  assert.equal(await spoolOwnerAlive(f), true); // we are the recorder
  // Same pid, previous boot: a recycled pid must not pin the spool.
  await writeFile(`${spoolPath(f)}.pid`, JSON.stringify({ pid: process.pid, boot: 12345 }));
  assert.equal(await spoolOwnerAlive(f), false);
  await rm(spoolPath(f), { force: true });
  await rm(`${spoolPath(f)}.pid`, { force: true });
});

test("owner liveness: EPERM (pid 1) counts as alive, dead pid does not", async () => {
  const f = cap("20260718-100001-bbbbbb");
  await appendSpool(f, [span(1)], skeleton());
  await writeFile(`${spoolPath(f)}.pid`, JSON.stringify({ pid: 1, boot: null }));
  assert.equal(await spoolOwnerAlive(f), true); // exists, not ours — alive
  await writeFile(`${spoolPath(f)}.pid`, JSON.stringify({ pid: 999999, boot: null }));
  assert.equal(await spoolOwnerAlive(f), false);
  await rm(spoolPath(f), { force: true });
  await rm(`${spoolPath(f)}.pid`, { force: true });
});

test("listCaptures finalizes a dead recorder's spool and heals stranded in_progress", async () => {
  // Dead-owner spool → finalized on list.
  const f = cap("20260718-100002-cccccc");
  await appendSpool(f, [span(1)], skeleton("dead"));
  await writeFile(`${spoolPath(f)}.pid`, JSON.stringify({ pid: 999999, boot: null }));
  // Stranded in_progress json with no spool → healed on list.
  const g = cap("20260718-100003-dddddd");
  const stranded = skeleton("stranded");
  stranded.spans.push(span(1));
  await writeFile(g, JSON.stringify(stranded));
  const listed = await listCaptures();
  const deadEntry = listed.find((c) => c.file === f);
  const strandedEntry = listed.find((c) => c.file === g);
  assert.equal(deadEntry?.in_progress, false);
  assert.equal(strandedEntry?.in_progress, false);
  const healed = JSON.parse(await readFile(g, "utf8"));
  assert.equal(healed.metadata.in_progress, undefined);
  assert.equal(healed.spans[0].ended_at, "2026-07-18T10:00:02.000Z"); // last span's end
  const finalized = JSON.parse(await readFile(f, "utf8"));
  assert.equal(finalized.metadata.in_progress, undefined);
  await assert.rejects(stat(spoolPath(f))); // spool consumed
});

test("stale commit lock is broken by age, not honored forever", async () => {
  const f = cap("20260718-100004-eeeeee");
  await appendSpool(f, [span(1)], skeleton());
  const lock = `${f}.lock`;
  await writeFile(lock, "999999");
  const old = new Date(Date.now() - 60_000);
  await utimes(lock, old, old);
  const n = await assembleSpool(f, { finalize: true, endedAt: "2026-07-18T11:00:00.000Z" });
  assert.equal(n, 1); // proceeded despite the dead holder's lock
  await assert.rejects(stat(lock)); // and cleaned it up
});

test("snapshot stamps the json with the spool's mtime so the freshness gate stays live", async () => {
  const f = cap("20260718-100005-ffffff");
  await appendSpool(f, [span(1)], skeleton());
  await assembleSpool(f); // snapshot
  const [sp, js] = [await stat(spoolPath(f)), await stat(f)];
  // json must NOT read newer than the spool it snapshotted.
  assert.ok(js.mtimeMs <= sp.mtimeMs + 1, `json ${js.mtimeMs} vs spool ${sp.mtimeMs}`);
});

test.after(async () => {
  await rm(home, { recursive: true, force: true });
});
