#!/usr/bin/env node
/**
 * Runs the protocol cases (testdata/protocol/README.md) through the viewer's
 * code in a real browser. test/protocol-cases.test.mjs runs the same code on
 * Node, whose Web Crypto is OpenSSL; a browser brings its own library, and
 * the viewer in a browser is what recipients use.
 *
 *   node scripts/check-protocol-browser.mjs
 *
 * BROWSER_BINARY names a Chromium-family browser; without it the script looks
 * for a Playwright Chromium, Chrome and Brave. Nothing leaves the machine:
 * the page and the cases are served from loopback.
 */
import { spawn } from "node:child_process";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { build } from "esbuild";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const playwright = path.join(os.homedir(), "Library/Caches/ms-playwright");
const shells = existsSync(playwright)
  ? readdirSync(playwright).filter((name) => name.startsWith("chromium_headless_shell-")).sort().reverse()
      .map((name) => path.join(playwright, name, "chrome-headless-shell-mac-arm64/chrome-headless-shell"))
  : [];
const executable = [process.env.BROWSER_BINARY, ...shells, "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"]
  .find((candidate) => candidate && existsSync(candidate));
if (!executable) {
  console.error("Set BROWSER_BINARY to a Chromium-family browser executable");
  process.exit(2);
}

const bundled = await build({
  stdin: { contents: 'export * from "./identity-protocol"; export * from "./identity-crypto"; export * from "./encryption";', resolveDir: path.join(root, "packages/viewer"), loader: "ts" },
  bundle: true,
  write: false,
  format: "iife",
  globalName: "viewer",
  logLevel: "silent",
});
const FILES = ["envelopes.json", "statements.json", "histories.json"];
const served = {
  "/": ["text/html", "<!doctype html><meta charset=utf-8><title>protocol cases</title><script src=/viewer.js></script>"],
  "/viewer.js": ["text/javascript", bundled.outputFiles[0].text],
  ...Object.fromEntries(FILES.map((name) => [`/${name}`, ["application/json", readFileSync(path.join(root, "testdata/protocol", name))]])),
};
const server = http.createServer((request, response) => {
  const [type, body] = served[request.url] ?? ["text/plain", ""];
  response.writeHead(served[request.url] ? 200 : 404, { "content-type": type });
  response.end(body);
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

// Runs in the page. It is the judging of test/protocol-cases.test.mjs.
async function judgeAll() {
  const base64 = (bytes) => btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(""));
  const bytes = (text) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
  const load = async (name) => (await fetch(`/${name}`)).json();
  const outcome = async (run) => {
    try {
      return { value: await run() };
    } catch (error) {
      return { error: String(error?.message ?? error) };
    }
  };
  const out = [];
  for (const item of await load("envelopes.json")) {
    const { value, error } = await outcome(() => viewer.decryptDocument(bytes(item.envelope), item.key));
    out.push({ file: "envelopes.json", name: item.name, verdict: error ? "refused" : "opens", error, read: error ? undefined : base64(value) });
  }
  for (const item of await load("statements.json")) {
    const { error } = await outcome(() => viewer.verify(item.kind, { payload: item.payload, signature: item.signature }, item.public));
    out.push({ file: "statements.json", name: item.name, verdict: error ? "refused" : "verified", error });
  }
  const { signed, cases } = await load("histories.json");
  for (const item of cases) {
    const events = item.events.map((id) => signed[id]);
    const { value, error } = await outcome(() =>
      item.vault ? viewer.verifyRecord({ events, vault: signed[item.vault] }, item.account) : viewer.verifyHistory(events, item.account),
    );
    if (error) {
      out.push({ file: "histories.json", name: item.name, verdict: "invalid", error });
      continue;
    }
    const { state, head, vault } = value;
    out.push({
      file: "histories.json",
      name: item.name,
      verdict: "valid",
      read: {
        head,
        seq: state.seq + 0,
        kind: state.kind,
        signer: state.signer,
        epoch: state.epoch,
        inbox: state.inbox || "",
        devices: state.devices.map((device) => ({ id: device.id, name: device.name })),
        ...(vault ? { revision: vault.revision } : {}),
      },
    });
  }
  return { agent: navigator.userAgent, out };
}

const scratch = await mkdtemp(path.join(os.tmpdir(), "slink-protocol-browser-"));
const browser = spawn(executable, ["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "--disable-component-update", "--disable-sync", "--remote-debugging-port=0", `--user-data-dir=${scratch}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
let socket;
let failures = 0;
try {
  const endpoint = await new Promise((resolve, reject) => {
    let text = "";
    const timeout = setTimeout(() => reject(new Error("the browser did not start")), 15_000);
    browser.stderr.on("data", (chunk) => {
      text += chunk;
      const match = text.match(/DevTools listening on (ws:\/\/\S+)/);
      if (match) {
        clearTimeout(timeout);
        resolve(match[1]);
      }
    });
    browser.on("error", reject);
  });
  // The first page can appear a moment after the debugging endpoint does.
  let target;
  for (let i = 0; i < 100 && !target; i++) {
    target = (await (await fetch(`http://127.0.0.1:${new URL(endpoint).port}/json/list`)).json()).find((entry) => entry.type === "page");
    if (!target) await new Promise((resolve) => setTimeout(resolve, 50));
  }
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => {
    socket.onopen = resolve;
    socket.onerror = reject;
  });
  let sequence = 0;
  const pending = new Map();
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    const call = pending.get(message.id);
    if (!call) return;
    pending.delete(message.id);
    if (message.error) call.reject(new Error(JSON.stringify(message.error)));
    else call.resolve(message.result);
  };
  const send = (method, params = {}) =>
    new Promise((resolve, reject) => {
      const id = ++sequence;
      pending.set(id, { resolve, reject });
      socket.send(JSON.stringify({ id, method, params }));
    });
  const evaluate = async (expression) => {
    const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  await send("Runtime.enable");
  await send("Page.enable");
  await send("Page.navigate", { url: `${origin}/` });
  for (let i = 0; i < 200 && !(await evaluate("typeof viewer === 'object' && !!crypto.subtle")); i++) await new Promise((resolve) => setTimeout(resolve, 50));
  const { agent, out } = await evaluate(`(${judgeAll})()`);

  const expected = Object.fromEntries(
    FILES.map((name) => {
      const file = JSON.parse(served[`/${name}`][1]);
      return [name, new Map((file.cases ?? file).map((item) => [item.name, item]))];
    }),
  );
  for (const got of out) {
    const item = expected[got.file].get(got.name);
    const want = typeof item.expect === "string" ? item.expect : item.expect.js;
    const read = item.plaintext ?? item.state?.js ?? item.state;
    const same = got.verdict !== want ? false : got.read === undefined || JSON.stringify(got.read) === JSON.stringify(read);
    if (same) continue;
    failures++;
    console.error(`${got.file}: ${got.name}\n  want ${want}, got ${got.verdict}${got.error ? ` (${got.error})` : ""}${got.verdict === want ? `, read ${JSON.stringify(got.read)}` : ""}`);
  }
  console.log(`${agent}\n${out.length} cases, ${failures} judged differently from what the cases say for the viewer`);
} finally {
  socket?.close();
  browser.kill();
  server.close();
  await rm(scratch, { recursive: true, force: true }).catch(() => {});
}
process.exit(failures ? 1 : 0);
