import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn } from "node:child_process";
import http from "node:http";
import { mkdtemp, readdir, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { validateRun } from "@session-link/format/validate";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

// A minimal but realistic Anthropic messages stream.
const SSE_EVENTS = [
  {
    type: "message_start",
    message: { id: "msg_1", model: "claude-test", role: "assistant", usage: { input_tokens: 3 } },
  },
  { type: "content_block_start", index: 0, content_block: { type: "text", text: "" } },
  { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: "Hello" } },
  { type: "content_block_delta", index: 0, delta: { type: "text_delta", text: " world" } },
  { type: "content_block_stop", index: 0 },
  { type: "message_delta", delta: { stop_reason: "end_turn" }, usage: { output_tokens: 2 } },
  { type: "message_stop" },
];
const sseBody = () =>
  SSE_EVENTS.map((e) => `event: ${e.type}\ndata: ${JSON.stringify(e)}`).join("\n\n") + "\n\n";

// The wrapped "agent": one streaming call, read to completion, exit at once.
// Exiting immediately after the last call is exactly what used to crash
// finish() with ENOENT (shared tmp path) or lose the final span.
const CHILD = `
await fetch(process.env.ANTHROPIC_BASE_URL + "/v1/messages", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ model: "claude-test", max_tokens: 16, stream: true,
    messages: [{ role: "user", content: "hi" }] }),
}).then((r) => r.text());
`;

// The same agent shape with concurrent calls — pre-fix, parallel record()
// flushes raced each other's shared tmp file (ENOENT mid-session).
const CHILD_PARALLEL = `
const call = () => fetch(process.env.ANTHROPIC_BASE_URL + "/v1/messages", {
  method: "POST",
  headers: { "content-type": "application/json" },
  body: JSON.stringify({ model: "claude-test", max_tokens: 16, stream: true,
    messages: [{ role: "user", content: "hi" }] }),
}).then((r) => r.text());
await Promise.all(Array.from({ length: 8 }, call));
`;

test("slink dev with 8 parallel calls: serialized flushes, all spans captured", async () => {
  const upstream = http.createServer((req, res) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    res.end(sseBody());
  });
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-e2e-par-"));

  try {
    const proc = spawn(
      process.execPath,
      [
        path.join(ROOT, "cli/slink.mjs"),
        "dev",
        "--name",
        "e2e-parallel",
        "--",
        process.execPath,
        "--input-type=module",
        "-e",
        CHILD_PARALLEL,
      ],
      {
        cwd: ROOT,
        timeout: 20_000,
        env: {
          ...process.env,
          SLINK_HOME: home,
          SLINK_UPSTREAM_ANTHROPIC: `http://127.0.0.1:${upstream.address().port}`,
        },
      },
    );
    let stderr = "";
    proc.stderr.on("data", (d) => (stderr += d));
    // "close", not "exit": stderr may not be fully flushed at exit — flake.
    const code = await new Promise((resolve) => proc.on("close", resolve));

    assert.equal(code, 0, `nonzero exit ${code}; stderr:\n${stderr}`);
    assert.match(stderr, /recorded 8 calls/);

    const runsDir = path.join(home, "runs");
    const files = (await readdir(runsDir)).filter((f) => f.endsWith(".json"));
    assert.equal(files.length, 1);
    const run = JSON.parse(await readFile(path.join(runsDir, files[0]), "utf8"));
    assert.deepEqual(validateRun(run), []);
    assert.equal(run.spans.filter((s) => s.type === "llm_call").length, 8);
    assert.equal(run.metadata.in_progress, undefined);
  } finally {
    upstream.close();
    await rm(home, { recursive: true, force: true });
  }
});

test("slink dev -- <fast command>: clean exit, valid capture, no tmp leftovers", async () => {
  const upstream = http.createServer((req, res) => {
    res.writeHead(200, { "content-type": "text/event-stream" });
    res.end(sseBody());
  });
  await new Promise((resolve) => upstream.listen(0, "127.0.0.1", resolve));
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-e2e-"));

  try {
    const proc = spawn(
      process.execPath,
      [
        path.join(ROOT, "cli/slink.mjs"),
        "dev",
        "--name",
        "e2e",
        "--",
        process.execPath,
        "--input-type=module",
        "-e",
        CHILD,
      ],
      {
        cwd: ROOT,
        timeout: 20_000,
        env: {
          ...process.env,
          SLINK_HOME: home,
          SLINK_UPSTREAM_ANTHROPIC: `http://127.0.0.1:${upstream.address().port}`,
        },
      },
    );
    let stderr = "";
    proc.stderr.on("data", (d) => (stderr += d));
    // "close", not "exit": stderr may not be fully flushed at exit — flake.
    const code = await new Promise((resolve) => proc.on("close", resolve));

    assert.equal(code, 0, `nonzero exit ${code}; stderr:\n${stderr}`);
    assert.match(stderr, /recorded 1 call/);

    const runsDir = path.join(home, "runs");
    const entries = await readdir(runsDir);
    assert.deepEqual(
      entries.filter((f) => f.includes(".tmp")),
      [],
      "no tmp files may survive",
    );
    const files = entries.filter((f) => f.endsWith(".json"));
    assert.equal(files.length, 1);

    const run = JSON.parse(await readFile(path.join(runsDir, files[0]), "utf8"));
    assert.deepEqual(validateRun(run), [], "proxy capture must be schema-valid");
    assert.equal(run.metadata.in_progress, undefined, "capture must be finalized");
    assert.equal(run.spans[0].ended_at !== undefined, true);

    const llm = run.spans.find((s) => s.type === "llm_call");
    assert.equal(llm.model.id, "claude-test");
    assert.equal(llm.metadata.stream, true);
    assert.equal(llm.output.messages[0].content[0].text, "Hello world");
    assert.deepEqual(llm.usage, { input_tokens: 3, output_tokens: 2 });
    assert.equal(llm.raw.response.content[0].text, "Hello world");
  } finally {
    upstream.close();
    await rm(home, { recursive: true, force: true });
  }
});
