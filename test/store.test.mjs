import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readdir, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { writeRun } from "../cli/store.mjs";

test("concurrent writeRun calls to one capture never collide or leak tmp files", async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "slink-store-"));
  const file = path.join(dir, "cap.json");
  try {
    // Pre-fix, all writers shared `${file}.tmp` and racing renames threw
    // ENOENT — the crash at the end of `slink dev -- <fast command>`.
    await Promise.all(
      Array.from({ length: 25 }, (_, i) => writeRun(file, { schema: "session/v0", i })),
    );
    const parsed = JSON.parse(await readFile(file, "utf8"));
    assert.equal(parsed.schema, "session/v0");
    const leftovers = (await readdir(dir)).filter((f) => f.includes(".tmp"));
    assert.deepEqual(leftovers, []);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

// ---------------------------------------------------------------- spool

import { appendFile } from "node:fs/promises";
import { appendSpool, assembleSpool, spoolPath } from "../cli/store.mjs";

const skeleton = () => ({
  schema: "session/v0",
  name: "spooled",
  created_at: "2026-07-18T10:00:00.000Z",
  source: { kind: "proxy", label: "t", fidelity: "exact" },
  metadata: { cwd: "/x", in_progress: true },
  spans: [{ id: "root", parent_id: null, type: "agent", name: "spooled", started_at: "2026-07-18T10:00:00.000Z" }],
});
const span = (i) => ({
  id: `s${i}`, parent_id: "root", type: "llm_call",
  model: { id: "m" },
  input: { messages: [{ role: "user", content: [{ type: "text", text: `call ${i}` }] }] },
  output: { messages: [] },
  started_at: "2026-07-18T10:00:01.000Z", ended_at: "2026-07-18T10:00:02.000Z",
});

test("spool: append + finalize assembles exactly the run the old writer produced", async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "slink-spool-"));
  const file = path.join(dir, "cap.json");
  try {
    await appendSpool(file, [span(1)], skeleton());
    await appendSpool(file, [span(2)], skeleton());
    await appendSpool(file, [span(3)], skeleton());
    const n = await assembleSpool(file, { finalize: true, endedAt: "2026-07-18T11:00:00.000Z" });
    assert.equal(n, 3);
    const got = JSON.parse(await readFile(file, "utf8"));
    const want = skeleton();
    delete want.metadata.in_progress;
    want.spans[0].ended_at = "2026-07-18T11:00:00.000Z";
    want.spans[0].status = "ok";
    want.spans.push(span(1), span(2), span(3));
    assert.deepEqual(got, want);
    // finalize consumes the spool and leaves no tmp litter
    await assert.rejects(readFile(spoolPath(file)));
    assert.deepEqual((await readdir(dir)).filter((f) => f.includes(".tmp")), []);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("spool: snapshot keeps in_progress, leaves the spool, and can finalize later", async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "slink-spool-"));
  const file = path.join(dir, "cap.json");
  try {
    await appendSpool(file, [span(1)], skeleton());
    assert.equal(await assembleSpool(file), 1);
    const snap = JSON.parse(await readFile(file, "utf8"));
    assert.equal(snap.metadata.in_progress, true);
    assert.equal(snap.spans[0].status, undefined);
    await readFile(spoolPath(file)); // still there
    await appendSpool(file, [span(2)], skeleton());
    assert.equal(await assembleSpool(file, { finalize: true, endedAt: "2026-07-18T11:00:00.000Z" }), 2);
    const fin = JSON.parse(await readFile(file, "utf8"));
    assert.equal(fin.spans.length, 3);
    assert.equal(fin.metadata.in_progress, undefined);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("spool: a crash-truncated trailing line is dropped, the rest survives", async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "slink-spool-"));
  const file = path.join(dir, "cap.json");
  try {
    await appendSpool(file, [span(1), span(2)], skeleton());
    await appendFile(spoolPath(file), '{"id":"s3","type":"llm_call","input":{"mess'); // no newline, cut mid-write
    assert.equal(await assembleSpool(file, { finalize: true, endedAt: "2026-07-18T11:00:00.000Z" }), 2);
    const got = JSON.parse(await readFile(file, "utf8"));
    assert.deepEqual(got.spans.map((s) => s.id), ["root", "s1", "s2"]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("spool: missing or span-less spools assemble to null and write nothing", async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "slink-spool-"));
  const file = path.join(dir, "cap.json");
  try {
    assert.equal(await assembleSpool(file), null);
    await appendFile(spoolPath(file), JSON.stringify(skeleton()) + "\n"); // header only — crashed before any call landed
    assert.equal(await assembleSpool(file), null);
    await assert.rejects(readFile(file)); // no .json materialized
    assert.deepEqual((await readdir(dir)).filter((f) => f.includes(".tmp")), []);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("spool: recreated after finalize gets a fresh header — never a bare-span spool", async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "slink-spool-"));
  const file = path.join(dir, "cap.json");
  try {
    await appendSpool(file, [span(1)], skeleton());
    await assembleSpool(file, { finalize: true, endedAt: "2026-07-18T11:00:00.000Z" });
    // A record landing late (recorder didn't see the finalize) must still
    // produce a valid, self-describing spool...
    await appendSpool(file, [span(2)], skeleton());
    const n = await assembleSpool(file, { finalize: true, endedAt: "2026-07-18T11:30:00.000Z" });
    assert.equal(n, 1);
    const got = JSON.parse(await readFile(file, "utf8"));
    assert.equal(got.schema, "session/v0"); // ...not schema-less garbage
    assert.deepEqual(got.spans.map((s) => s.id), ["root", "s2"]);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("spool: U+2028/U+2029 in captured text stays one line, one span", async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "slink-spool-"));
  const file = path.join(dir, "cap.json");
  try {
    const s = span(1);
    s.input.messages[0].content[0].text = "line\u2028sep\u2029chars";
    await appendSpool(file, [s], skeleton());
    assert.equal(await assembleSpool(file, { finalize: true, endedAt: "2026-07-18T11:00:00.000Z" }), 1);
    const got = JSON.parse(await readFile(file, "utf8"));
    assert.equal(got.spans[1].input.messages[0].content[0].text, "line\u2028sep\u2029chars");
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("spool: a first line that isn't a run skeleton is refused, not promoted", async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "slink-spool-"));
  const file = path.join(dir, "cap.json");
  try {
    await appendFile(spoolPath(file), JSON.stringify(span(1)) + "\n" + JSON.stringify(span(2)) + "\n");
    assert.equal(await assembleSpool(file, { finalize: true }), null);
    await assert.rejects(readFile(file)); // nothing fabricated
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});

test("router: busy sessions are not swept or rolled over, however stale", async () => {
  const { SessionRouter } = await import("../cli/tap.mjs");
  let t = 0;
  const finalized = [];
  const r = new SessionRouter({
    idleMs: 100,
    now: () => t,
    newSession: (name) => ({ name, pending: 0 }),
    onFinalize: (s) => finalized.push(s),
    isBusy: (s) => s.pending > 0,
  });
  const s1 = r.route("k");
  s1.pending = 1; // a call is mid-stream
  t = 500; // way past idle
  r.sweep();
  assert.deepEqual(finalized, []); // busy — spared
  assert.equal(r.route("k"), s1); // and reused, not rolled over
  s1.pending = 0;
  t = 1000;
  r.sweep();
  assert.deepEqual(finalized, [s1]); // idle and quiet — finalized
});
