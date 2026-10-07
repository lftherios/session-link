#!/usr/bin/env node
// Real-browser regression check using fictional data and an isolated home.
import assert from "node:assert/strict";
import { spawn, spawnSync } from "node:child_process";
import { mkdtemp, readFile, writeFile, readdir, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { tmpdir } from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const scratch = await mkdtemp(path.join(tmpdir(), "slink-security-"));
const home = path.join(scratch, "home"), binary = process.env.SLINK_BINARY ?? "/tmp/session-link-slink";
const env = { ...process.env, SLINK_HOME: home, SLINK_SERVER: "http://127.0.0.1:1", SLINK_API_KEY: "" };
process.umask(0o022);
const imported = spawnSync(binary, ["import", "--from", "pi", "--session", path.join(root, "testdata/import/pi/basic/input.session.jsonl")], { cwd: root, env, encoding: "utf8" });
assert.equal(imported.status, 0, imported.stderr);
const runs = path.join(home, "runs"), file = (await readdir(runs)).find(f => f.endsWith(".json"));
if (process.platform !== "win32") {
  assert.equal((await stat(home)).mode & 0o777, 0o700);
  assert.equal((await stat(runs)).mode & 0o777, 0o700);
  assert.equal((await stat(path.join(runs, file))).mode & 0o777, 0o600);
}
let localURL = "", hits = [], cli, browser, socket;
const collector = createServer((req, res) => {
  if (req.url === "/frame") { res.setHeader("content-type", "text/html"); res.end(`<iframe src="${localURL}" width="1250" height="950"></iframe>`); return; }
  if (req.url?.startsWith("/pixel")) hits.push({ url: req.url, headers: req.headers });
  res.setHeader("content-type", "image/png");
  res.end(Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+/ncoAAAAASUVORK5CYII=", "base64"));
});
await new Promise((resolve, reject) => { collector.once("error", reject); collector.listen(0, "127.0.0.1", resolve); });
const remote = `http://127.0.0.1:${collector.address().port}`;
const doc = JSON.parse(await readFile(path.join(root, "packages/format/examples/chat.json"), "utf8"));
doc.spans[0].output.messages[0].content = [
  { type: "text", text: `![Security review image](${remote}/pixel?fictional=PRIVATE_TEST_MARKER)` },
  { type: "image", url: `${remote}/pixel?fictional=STRUCTURED_MARKER` },
];
const fixture = path.join(scratch, "fixture.json"); await writeFile(fixture, JSON.stringify(doc));
const waitOutput = (child, stream, pattern) => new Promise((resolve, reject) => {
  let output = ""; const timer = setTimeout(() => reject(new Error(`Startup timed out: ${output.slice(-500)}`)), 15000);
  child[stream].on("data", chunk => { output += chunk; const match = output.match(pattern); if (match) { clearTimeout(timer); resolve(match[0]); } });
  child.once("error", error => { clearTimeout(timer); reject(error); });
  child.once("exit", code => { clearTimeout(timer); reject(new Error(`Exited ${code}: ${output.slice(-500)}`)); });
});
try {
  cli = spawn(binary, ["view", "--session", fixture, "--no-browser"], { cwd: root, env, stdio: ["ignore", "pipe", "pipe"] });
  localURL = await waitOutput(cli, "stdout", /http:\/\/127\.0\.0\.1:\d+\/p\/[^\s]+/);
  const url = new URL(localURL), base = url.origin, id = url.pathname.split("/").pop();
  const key = new URLSearchParams(url.hash.slice(1)).get("access"); assert.match(key, /^[\w-]{43}$/);
  for (const endpoint of [`/api/document/${id}`, "/api/login/status", "/settings"]) {
    assert.equal((await fetch(base + endpoint, { headers: { "x-slink": "1" } })).status, 403);
    assert.equal((await fetch(base + endpoint, { headers: { "x-slink-access": "wrong" } })).status, 403);
  }
  assert.equal((await fetch(base + "/api/stop", { method: "POST", headers: { "x-slink": "1" } })).status, 403);
  const response = await fetch(base + `/api/document/${id}`, { headers: { "x-slink-access": key } });
  assert.equal(response.status, 200); assert.match(await response.text(), /PRIVATE_TEST_MARKER/);
  assert.equal(response.headers.get("x-frame-options"), "DENY");
  assert.match(response.headers.get("content-security-policy"), /frame-ancestors 'none'/);

  browser = spawn(process.env.BROWSER_BINARY ?? "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser", ["--headless", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "--disable-component-update", "--disable-sync", `--user-data-dir=${path.join(scratch, "browser")}`, "--remote-debugging-port=0", "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
  const endpoint = await waitOutput(browser, "stderr", /ws:\/\/[^\s]+/);
  const targets = async () => (await fetch(`http://127.0.0.1:${new URL(endpoint).port}/json/list`)).json();
  let target;
  for (let i = 0; i < 100 && !target; i++) { target = (await targets()).find(t => t.type === "page"); if (!target) await new Promise(r => setTimeout(r, 50)); }
  socket = new WebSocket(target.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let seq = 0; const pending = new Map(), requests = [];
  socket.onmessage = ({ data }) => {
    const msg = JSON.parse(data);
    if (msg.id) { const p = pending.get(msg.id); pending.delete(msg.id); msg.error ? p.reject(msg.error) : p.resolve(msg.result); }
    if (msg.method === "Network.requestWillBeSent") requests.push(msg.params.request);
  };
  const send = (method, params = {}) => new Promise((resolve, reject) => { const id = ++seq; pending.set(id, { resolve, reject }); socket.send(JSON.stringify({ id, method, params })); });
  const evaluate = async expression => { const result = await send("Runtime.evaluate", { expression, returnByValue: true, awaitPromise: true, userGesture: true }); if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails)); return result.result.value; };
  const waitFor = async expression => { for (let i = 0; i < 100; i++) { if (await evaluate(expression)) return; await new Promise(r => setTimeout(r, 50)); } throw new Error(`Timed out: ${expression}; page: ${await evaluate("document.body.innerText.slice(0,500)")}`); };
  await send("Page.enable"); await send("Network.enable");
  await send("Page.navigate", { url: base + url.pathname });
  await waitFor("document.body.innerText.includes('Open the complete local URL')");
  assert.equal(await evaluate("!!window.__RUN__"), false);
  // Pasting the complete URL into the locked page must unlock it too.
  await send("Page.navigate", { url: localURL });
  await waitFor("document.querySelectorAll('a').length > 2 && document.body.innerText.includes('Open external image')");
  assert.equal(hits.length, 0, "opening a transcript must not fetch its image URLs");
  assert.equal(await evaluate("Array.from(document.images).some(i => /^https?:/.test(i.src))"), false);
  assert.equal(await evaluate("fetch('/api/login/status',{headers:{'x-slink':'1'}}).then(r=>r.status)"), 200);
  await evaluate("delete window.__RUN__"); await send("Page.reload");
  await waitFor("!!window.__RUN__ && document.body.innerText.includes('Open external image')");
  const sessions = await evaluate("Array.from(document.querySelectorAll('a')).find(a=>new URL(a.href).pathname==='/').href");
  assert.equal(new URLSearchParams(new URL(sessions).hash.slice(1)).get("access"), key);
  // A copied link must work without any existing sessionStorage.
  await evaluate("sessionStorage.clear()");
  await send("Page.navigate", { url: sessions });
  await waitFor("!!document.querySelector('#stop-viewer') || document.body.innerText.includes('Stop viewer')");
  assert.equal(await evaluate("new URLSearchParams(location.hash.slice(1)).get('access')"), key);
  await send("Page.navigate", { url: localURL });
  await waitFor("document.body.innerText.includes('Open external image')");
  await evaluate("Array.from(document.querySelectorAll('a')).find(a=>a.textContent==='Open external image').click()");
  for (let i = 0; i < 100 && !hits.length; i++) await new Promise(r => setTimeout(r, 50));
  assert.equal(hits.length, 1, "explicit image navigation should work");
  assert.equal(hits[0].headers.referer, undefined);
  assert.equal(hits[0].headers["x-slink-access"], undefined);
  await send("Page.navigate", { url: remote + "/frame" });
  await waitFor("!!document.querySelector('iframe')");
  for (let i = 0; i < 100; i++) {
    const tree = await send("Page.getFrameTree");
    if (tree.frameTree.childFrames?.[0]?.frame.url === "chrome-error://chromewebdata/") break;
    if (i === 99) throw new Error("frame navigation was not blocked");
    await new Promise(r => setTimeout(r, 50));
  }
  for (const request of requests) {
    assert.equal(new URL(request.url).search.includes(key), false);
    if (!request.url.startsWith(base + "/")) assert.equal(request.headers["x-slink-access"], undefined);
  }
  console.log("PASS: private captures, authenticated reads/actions/navigation, reload and copied links, blocked framing, and image privacy");
} finally { socket?.close(); browser?.kill(); cli?.kill("SIGTERM"); collector.closeAllConnections(); collector.close(); }
console.log(`Browser artifacts: ${scratch}`);
