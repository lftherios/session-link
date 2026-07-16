import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn } from "node:child_process";
import { mkdtemp, readdir, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const TRANSCRIPT = [
  JSON.stringify({ type: "summary", summary: "e2e import" }),
  JSON.stringify({
    sessionId: "s1",
    cwd: "/tmp",
    type: "user",
    timestamp: "2026-07-16T10:00:00.000Z",
    message: { role: "user", content: "hi" },
  }),
  JSON.stringify({
    sessionId: "s1",
    cwd: "/tmp",
    type: "assistant",
    timestamp: "2026-07-16T10:00:01.000Z",
    message: { role: "assistant", model: "claude-test", content: [{ type: "text", text: "hello" }] },
  }),
].join("\n");

function runSlink(args, env) {
  const proc = spawn(process.execPath, [path.join(ROOT, "cli/slink.mjs"), ...args], {
    cwd: ROOT,
    timeout: 15_000,
    env: { ...process.env, ...env },
  });
  let stderr = "";
  proc.stderr.on("data", (d) => (stderr += d));
  return new Promise((resolve) => proc.on("close", (code) => resolve({ code, stderr })));
}

// The --session flag shipped broken for its whole life (missing from the
// value-flag list, crashed on readFile(true)) — this gates the CLI leg, not
// just transcriptToRun.
test("slink import --session <file> works through the real CLI", async () => {
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-imp-"));
  const sess = path.join(home, "sess.jsonl");
  await writeFile(sess, TRANSCRIPT);
  try {
    const { code, stderr } = await runSlink(["import", "--session", sess], { SLINK_HOME: home });
    assert.equal(code, 0, stderr);
    assert.match(stderr, /imported "e2e import"/);
    const files = (await readdir(path.join(home, "runs"))).filter((f) => f.endsWith(".json"));
    assert.equal(files.length, 1);
    const run = JSON.parse(await readFile(path.join(home, "runs", files[0]), "utf8"));
    assert.equal(run.source.fidelity, "reconstructed");
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test("slink import <file> positional form works", async () => {
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-imp-"));
  const sess = path.join(home, "sess.jsonl");
  await writeFile(sess, TRANSCRIPT);
  try {
    const { code, stderr } = await runSlink(["import", sess], { SLINK_HOME: home });
    assert.equal(code, 0, stderr);
    assert.match(stderr, /imported "e2e import"/);
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test("slink import --session with no value dies loudly, not silently", async () => {
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-imp-"));
  try {
    const { code, stderr } = await runSlink(["import", "--session"], { SLINK_HOME: home });
    assert.equal(code, 1);
    assert.match(stderr, /--session requires a value/);
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});

test("slink import on a missing file exits 1 with a clean error", async () => {
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-imp-"));
  try {
    const { code, stderr } = await runSlink(["import", "--session", "/nope.jsonl"], { SLINK_HOME: home });
    assert.equal(code, 1);
    assert.match(stderr, /cannot read \/nope\.jsonl/);
    assert.doesNotMatch(stderr, /at (async )?\w+/, "no stack trace");
  } finally {
    await rm(home, { recursive: true, force: true });
  }
});
