#!/usr/bin/env node
// Invoked by the hosted service's smoke-encrypted.mjs against its fixture
// server, with SLINK_BINARY naming a built slink, Go or Rust. The binary signs
// in through the service's email approval, publishes a session with the key it
// saved, and a browser opens the link it printed: check-encrypted-browser.mjs
// runs on that link in place of the smoke's own. Then the binary takes the
// link offline and the service answers 410 for it.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { readFileSync, statSync } from "node:fs";
import { mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const base = process.env.SLINK_BROWSER_BASE, mail = process.env.SLINK_BROWSER_MAIL, binary = process.env.SLINK_BINARY;
if (!base || !mail || !binary) throw new Error("Run through smoke-encrypted.mjs with SLINK_BINARY set and SLINK_BROWSER_SCRIPT set to this file");
const dir = await mkdtemp(path.join(os.tmpdir(), "slink-publish-browser-"));
// A home of its own, and no PATH: no browser opens and no clipboard is written.
const env = { HOME: dir, USERPROFILE: dir, SLINK_HOME: path.join(dir, "home"), PATH: path.join(dir, "no-programs") };
if (process.env.SystemRoot) env.SystemRoot = process.env.SystemRoot;

// `slink login`: approve the URL and code it prints as a person signed in by
// email would, and wait for it to save the key.
const login = spawn(binary, ["login", "--server", base], { env, stdio: ["ignore", "pipe", "pipe"] });
let said = "";
const exited = new Promise((resolve) => login.on("exit", resolve));
const [, url, userCode] = await new Promise((resolve, reject) => {
  login.stderr.on("data", (chunk) => {
    said += chunk;
    const found = said.match(/Opening (\S+)\n {2}confirm this code in the browser: (\S+)/);
    if (found) resolve(found);
  });
  exited.then(() => reject(new Error(`slink login stopped: ${said}`)));
});
assert.ok(url.startsWith(`${base}/cli/`), `the sign-in page is on another server: ${url}`);
const code = url.split("/").at(-1);
const form = (route, body, cookie = "") => fetch(base + route, { method: "POST", redirect: "manual", headers: { "content-type": "application/x-www-form-urlencoded", origin: base, cookie }, body: new URLSearchParams(body) });
const cookies = (response) => response.headers.getSetCookie().map((cookie) => cookie.split(";")[0]).join("; ");
let response = await form("/api/auth/email/start", { email: "publisher@example.test", next: `/cli/${code}` });
const binding = cookies(response), attempt = response.headers.get("location").split("/").at(-1);
const emailed = (await (await fetch(mail)).json()).text.match(/code is (\d{8})/)[1];
response = await form("/api/auth/email/verify", { id: attempt, code: emailed }, binding);
response = await form(`/api/auth/cli/${code}/approve`, { user_code: userCode }, cookies(response));
assert.equal(response.status, 303);
assert.equal(await exited, 0, said);
assert.match(said, /✓ logged in as @publisher@example\.test to /);
const config = path.join(env.SLINK_HOME, "config.json");
if (process.platform !== "win32") assert.equal(statSync(config).mode & 0o777, 0o600);
assert.equal(JSON.parse(readFileSync(config, "utf8")).server, base);

// Publish with the saved sign-in: no --key and no --server.
const file = path.join(dir, "session.json");
await writeFile(file, JSON.stringify({ schema: "session/v0", created_at: "2026-09-12T00:00:00Z", name: "PRIVATE_TITLE_SMOKE_SENTINEL", spans: [{ id: "root", type: "agent", raw: { text: "PRIVATE_TEXT_SMOKE_SENTINEL" } }] }));
const pushed = spawnSync(binary, ["push", file, "--yes"], { env, encoding: "utf8" });
assert.equal(pushed.status, 0, pushed.stderr);
const link = pushed.stdout.trim();
assert.ok(link.startsWith(`${base}/s/`), link);
const checked = spawnSync(process.execPath, [path.join(path.dirname(fileURLToPath(import.meta.url)), "check-encrypted-browser.mjs")], {
  env: { ...process.env, SLINK_BROWSER_LINK: link }, stdio: "inherit",
});
assert.equal(checked.status, 0, "the browser check failed on the published link");

const deleted = spawnSync(binary, ["delete", link, "--yes"], { env, encoding: "utf8" });
assert.equal(deleted.status, 0, deleted.stderr);
assert.equal((await fetch(`${base}/api/shares/${new URL(link).pathname.split("/").at(-1)}`)).status, 410);
console.log(`✓ ${path.basename(binary)}: login by email approval, push with the saved key, browser decryption, delete and 410 (${dir})`);
