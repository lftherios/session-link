import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn } from "node:child_process";
import http from "node:http";
import { mkdtemp, readdir, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

// A minimal OpenAI upstream: one non-streaming chat.completion.
function upstream() {
  const server = http.createServer((req, res) => {
    if (req.method === "POST" && req.url === "/v1/chat/completions") {
      req.on("data", () => {});
      req.on("end", () => {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({
          id: "cc", object: "chat.completion", model: "gpt-4o-mini",
          choices: [{ index: 0, message: { role: "assistant", content: "ok" }, finish_reason: "stop" }],
          usage: { prompt_tokens: 5, completion_tokens: 1, total_tokens: 6 },
        }));
      });
    } else {
      res.writeHead(404);
      res.end();
    }
  });
  return new Promise((r) => server.listen(0, "127.0.0.1", () => r({ server, port: server.address().port })));
}

function callTap(tapPort, sessionKey, text) {
  const headers = { "content-type": "application/json" };
  if (sessionKey) headers["x-slink-session"] = sessionKey;
  return fetch(`http://127.0.0.1:${tapPort}/openai/v1/chat/completions`, {
    method: "POST",
    headers,
    body: JSON.stringify({ model: "gpt-4o-mini", messages: [{ role: "user", content: text }] }),
  }).then((r) => r.text());
}

test("slink tap: segments concurrent sessions by header into separate captures", async () => {
  const up = await upstream();
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-tap-"));
  const tap = spawn(process.execPath, [path.join(ROOT, "cli/slink.mjs"), "tap", "--port", "0"], {
    env: { ...process.env, SLINK_HOME: home, SLINK_UPSTREAM_OPENAI: `http://127.0.0.1:${up.port}` },
  });
  let stderr = "";
  tap.stderr.on("data", (d) => (stderr += d));

  try {
    // Read the ephemeral port the tap bound, from its startup log.
    const start = Date.now();
    let port;
    while (Date.now() - start < 4000) {
      const m = stderr.match(/127\.0\.0\.1:(\d+)/);
      if (m) { port = Number(m[1]); break; }
      await sleep(30);
    }
    assert.ok(port, `tap never reported a port; stderr:\n${stderr}`);

    // Two turns keyed to session "aaa", one to "bbb" — distinct sessions.
    await callTap(port, "aaa", "first in aaa");
    await callTap(port, "aaa", "second in aaa");
    await callTap(port, "bbb", "only in bbb");
    await sleep(300); // let the async recordCalls settle before shutdown

    tap.kill("SIGINT");
    await new Promise((resolve) => tap.on("close", resolve));

    const files = (await readdir(path.join(home, "runs"))).filter((f) => f.endsWith(".json"));
    assert.equal(files.length, 2, `expected two captures, got ${files.length}`);
    const runs = await Promise.all(files.map(async (f) => JSON.parse(await readFile(path.join(home, "runs", f), "utf8"))));
    const { validateRun } = await import("@session-link/format/validate");
    for (const run of runs) assert.deepEqual(validateRun(run), [], `invalid: ${JSON.stringify(run.spans?.length)}`);

    const llmCounts = runs.map((r) => r.spans.filter((s) => s.type === "llm_call").length).sort();
    assert.deepEqual(llmCounts, [1, 2], "one session has 2 calls, the other 1");
    // the two-call session should be named from its first user message
    const twoCall = runs.find((r) => r.spans.filter((s) => s.type === "llm_call").length === 2);
    assert.equal(twoCall.name, "first in aaa");
    assert.equal(twoCall.source.kind, "proxy");
    assert.equal(twoCall.source.fidelity, "exact");
  } finally {
    if (!tap.killed) tap.kill("SIGKILL");
    up.server.close();
    await rm(home, { recursive: true, force: true });
  }
});
