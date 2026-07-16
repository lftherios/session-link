import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn } from "node:child_process";
import http from "node:http";
import { mkdtemp, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");

function runLogin(server, home) {
  const proc = spawn(process.execPath, [path.join(ROOT, "cli/slink.mjs"), "login", "--server", server], {
    cwd: ROOT,
    timeout: 15_000,
    // No BROWSER/DISPLAY, and openBrowser swallows spawn errors — the poll
    // loop is what we're exercising, not the browser.
    env: { ...process.env, SLINK_HOME: home },
  });
  let stderr = "";
  proc.stderr.on("data", (d) => (stderr += d));
  return new Promise((resolve) => proc.on("close", (code) => resolve({ code, stderr })));
}

test("slink login: 503 (auth not configured) exits 1 with the --key hint", async () => {
  const server = http.createServer((req, res) => {
    res.writeHead(503, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: { code: "auth_not_configured", message: "Browser login is not configured; use `slink login --key rk_…`." } }));
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-login-"));
  try {
    const { code, stderr } = await runLogin(`http://127.0.0.1:${server.address().port}`, home);
    assert.equal(code, 1);
    assert.match(stderr, /--key/);
    await assert.rejects(readFile(path.join(home, "config.json")), "no config written on failure");
  } finally {
    server.close();
    await rm(home, { recursive: true, force: true });
  }
});

test("slink login: mainline browser flow writes the account key to config", async () => {
  let polls = 0;
  const server = http.createServer((req, res) => {
    const u = new URL(req.url, "http://x");
    if (req.method === "POST" && u.pathname === "/api/auth/cli") {
      res.writeHead(200, { "content-type": "application/json" });
      res.end(JSON.stringify({ code: "logincode", user_code: "WXYZ-1234", url: "http://x/cli/logincode" }));
    } else if (u.pathname === "/api/auth/cli/logincode") {
      // Pending on the first poll, granted on the second — exercises the loop.
      polls += 1;
      if (polls < 2) {
        res.writeHead(202, { "content-type": "application/json" });
        res.end(JSON.stringify({ status: "pending" }));
      } else {
        res.writeHead(200, { "content-type": "application/json" });
        res.end(JSON.stringify({ key: "rk_test_account_key", login: "alice" }));
      }
    } else {
      res.writeHead(404);
      res.end();
    }
  });
  await new Promise((r) => server.listen(0, "127.0.0.1", r));
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-login-"));
  const srv = `http://127.0.0.1:${server.address().port}`;
  try {
    const { code, stderr } = await runLogin(srv, home);
    assert.equal(code, 0, stderr);
    assert.match(stderr, /confirm this code in the browser: .*WXYZ-1234/s);
    assert.match(stderr, /logged in as @alice/);
    const config = JSON.parse(await readFile(path.join(home, "config.json"), "utf8"));
    assert.equal(config.api_key, "rk_test_account_key");
    assert.equal(config.server, srv);
  } finally {
    server.close();
    await rm(home, { recursive: true, force: true });
  }
});
