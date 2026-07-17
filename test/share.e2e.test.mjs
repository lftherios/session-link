import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn } from "node:child_process";
import http from "node:http";
import { mkdtemp, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

const TRANSCRIPT = [
  JSON.stringify({ type: "summary", summary: "share e2e" }),
  JSON.stringify({ sessionId: "s1", cwd: "/tmp", type: "user", timestamp: "2026-07-17T10:00:00.000Z", message: { role: "user", content: "hi" } }),
  JSON.stringify({ sessionId: "s1", cwd: "/tmp", type: "assistant", timestamp: "2026-07-17T10:00:01.000Z", message: { role: "assistant", model: "claude-test", content: [{ type: "text", text: "hello" }] } }),
].join("\n");

// Minimal ingest server that mimics POST /api/runs.
function mockServer() {
  const received = [];
  const server = http.createServer((req, res) => {
    if (req.method === "POST" && req.url === "/api/runs") {
      let body = "";
      req.on("data", (d) => (body += d));
      req.on("end", () => {
        received.push(body);
        res.writeHead(201, { "content-type": "application/json" });
        res.end(JSON.stringify({ id: "9f3kx2mvq7wt", url: "http://mock/r/9f3kx2mvq7wt", hash: "sha256:abc", deduplicated: false }));
      });
    } else {
      res.writeHead(404);
      res.end();
    }
  });
  return new Promise((resolve) => server.listen(0, "127.0.0.1", () => resolve({ server, received, port: server.address().port })));
}

function runSlink(argv, env) {
  const proc = spawn(process.execPath, [path.join(ROOT, "cli/slink.mjs"), ...argv], { cwd: ROOT, timeout: 15_000, env: { ...process.env, ...env } });
  let stdout = "";
  let stderr = "";
  proc.stdout.on("data", (d) => (stdout += d));
  proc.stderr.on("data", (d) => (stderr += d));
  return new Promise((resolve) => proc.on("close", (code) => resolve({ code, stdout, stderr })));
}

test("slink share: imports the latest session and publishes it in one step", async () => {
  const { server, received, port } = await mockServer();
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-share-"));
  const sess = path.join(home, "sess.jsonl");
  await writeFile(sess, TRANSCRIPT);
  try {
    const { code, stdout, stderr } = await runSlink(
      ["share", "--from", "claude-code", "--session", sess, "--yes"],
      { SLINK_HOME: home, SLINK_SERVER: `http://127.0.0.1:${port}` },
    );
    assert.equal(code, 0, stderr);
    assert.match(stderr, /imported "share e2e" from claude-code/); // the import leg ran
    assert.match(stderr, /published/); // the publish leg ran
    assert.equal(stdout.trim(), "http://mock/r/9f3kx2mvq7wt"); // the URL is the one pipeable line
    assert.equal(received.length, 1, "exactly one run uploaded");
    const run = JSON.parse(received[0]);
    assert.equal(run.source.harness, "claude-code");
    assert.deepEqual([], (await import("@session-link/format/validate")).validateRun(run));
  } finally {
    server.close();
    await rm(home, { recursive: true, force: true });
  }
});
