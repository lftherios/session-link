#!/usr/bin/env node
// Capture the built product, using fictional sessions and an isolated local home.
// Run npm run build:viewer and rebuild slink first. See assets/product/README.md.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdir, mkdtemp, realpath, writeFile, utimes } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import os from "node:os";
import { sessions } from "../testdata/landing/sessions.mjs";
import { validateRun } from "../packages/format/validate.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const output = path.resolve(process.env.SCREENSHOT_DIR ?? path.join(root, "assets/product"));
const workspace = await realpath(await mkdtemp(path.join(os.tmpdir(), "slink-product-")));
const project = path.join(workspace, "sample-project");
const localHome = path.join(workspace, "slink");
await mkdir(project, { recursive: true });
await mkdir(path.join(localHome, "runs"), { recursive: true });
await mkdir(output, { recursive: true });
for (const [index, session] of sessions.entries()) {
  assert.deepEqual(validateRun(session), [], session.name);
  const file = path.join(localHome, "runs", `${session.metadata.session_id}.json`);
  await writeFile(file, JSON.stringify({ ...session, metadata: { ...session.metadata, cwd: project } }));
  const modified = new Date(Date.now() - (index === 0 ? 5 * 60000 : index === 1 ? 50 * 60000 : 86400000));
  await utimes(file, modified, modified);
}

const startup = (child, stream, pattern) => new Promise((resolve, reject) => {
  let output = "";
  const timer = setTimeout(() => reject(new Error(`Startup timed out: ${output.slice(-1000)}`)), 15000);
  child[stream].on("data", chunk => {
    output += chunk;
    const match = output.match(pattern);
    if (match) { clearTimeout(timer); resolve(match[0]); }
  });
  child.once("error", error => { clearTimeout(timer); reject(error); });
  child.once("exit", code => { clearTimeout(timer); reject(new Error(`Process exited ${code}: ${output.slice(-1000)}`)); });
});
let cli, browser, socket;
try {
  cli = spawn(process.env.SLINK_BINARY ?? "/tmp/session-link-landing-slink", ["view", "--pick", "--no-browser"], {
    cwd: project,
    env: { ...process.env, SLINK_HOME: localHome, SLINK_SERVER: "http://127.0.0.1:1", SLINK_API_KEY: "" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  const base = await startup(cli, "stdout", /http:\/\/127\.0\.0\.1:\d+/);
  browser = spawn(process.env.BROWSER_BINARY ?? "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser", [
    "--headless", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
    "--disable-background-networking", "--disable-component-update", "--disable-sync",
    "--remote-debugging-port=0", `--user-data-dir=${path.join(workspace, "browser")}`, "about:blank",
  ], { stdio: ["ignore", "ignore", "pipe"] });
  const endpoint = await startup(browser, "stderr", /ws:\/\/[^\s]+/);
  const targets = await (await fetch(`http://127.0.0.1:${new URL(endpoint).port}/json/list`)).json();
  socket = new WebSocket(targets.find(target => target.type === "page").webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let sequence = 0;
  const pending = new Map(), errors = [];
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (message.id) {
      const call = pending.get(message.id); pending.delete(message.id);
      message.error ? call.reject(new Error(JSON.stringify(message.error))) : call.resolve(message.result);
    }
    if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.text);
  };
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++sequence; pending.set(id, { resolve, reject }); socket.send(JSON.stringify({ id, method, params }));
  });
  const evaluate = async expression => {
    const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };
  const waitFor = async expression => {
    for (let i = 0; i < 200; i++) {
      if (await evaluate(expression)) return;
      await new Promise(resolve => setTimeout(resolve, 50));
    }
    const page = await evaluate("JSON.stringify({url:location.href,title:document.title,rows:document.querySelectorAll('.row').length,empty:document.querySelector('.empty-state')?.textContent,status:document.querySelector('#result')?.textContent})");
    throw new Error(`Timed out: ${expression}; ${page}`);
  };
  const click = selector => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const field = (selector, value) => evaluate(`(()=>{
    const el=document.querySelector(${JSON.stringify(selector)});
    const proto=el instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;
    Object.getOwnPropertyDescriptor(proto,'value').set.call(el,${JSON.stringify(value)});
    el.dispatchEvent(new Event('input',{bubbles:true}));
  })()`);
  const capture = async (name, width = 1280, height = 940) => {
    await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: 1, mobile: false });
    await evaluate("document.activeElement?.blur();document.fonts.ready.then(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))))");
    assert.equal(await evaluate("document.documentElement.scrollWidth > innerWidth"), false, `${name}: horizontal overflow`);
    const { data } = await send("Page.captureScreenshot", { format: "webp", quality: 92 });
    await writeFile(path.join(output, `${name}.webp`), Buffer.from(data, "base64"));
    console.log(`Captured ${name}.webp (${width} × ${height})`);
  };
  await send("Runtime.enable"); await send("Page.enable");
  await send("Emulation.setTimezoneOverride", { timezoneId: "Europe/Berlin" });
  await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 940, deviceScaleFactor: 1, mobile: false });
  await send("Page.navigate", { url: base });
  await waitFor("document.querySelectorAll('.row[data-source]').length===3");
  await capture("sessions", 1100, 680);
  await click('[data-source="0"]');
  await waitFor("!!document.querySelector('.sv-response-body table')");
  assert.match(await evaluate("document.querySelector('.sv-position').textContent"), /4 of 4/);
  await capture("focused-mobile", 390, 1000);
  await capture("focused", 1280, 940);
  await click(".sv-share");
  await waitFor("document.querySelectorAll('.sv-included-item').length===2");
  await click(".sv-author-fields>summary");
  await field('[aria-label="View title"]', "Try a sample project before asking for setup");
  await field('[aria-label="Your comment"]', "Could we try this in the next onboarding iteration? The comparison and proposed experiment are included below.");
  await capture("prepare-view", 1100, 960);
  await click('[aria-label="Close share panel"]');
  await field('[aria-label="Search session content"]', "workspace");
  await evaluate("[...document.querySelectorAll('.sv-search-scope button')].find(b=>b.textContent==='Whole session').click()");
  await waitFor("document.querySelectorAll('#sv-search-results .sv-outline-list>button').length>2");
  await capture("search", 1280, 940);
  await send("Page.navigate", { url: base });
  await waitFor("document.querySelectorAll('.row[data-source]').length===3");
  await click('[data-source="1"]');
  await waitFor("!!document.querySelector('.sv-response-body') && document.querySelector('.sv-position').textContent.includes('3 of 3')");
  await click(".sv-support>summary");
  await waitFor("document.querySelector('.sv-support').open");
  await capture("agent-activity", 1280, 1200);
  assert.deepEqual(errors, [], "browser runtime errors");
  const manifest = { captured_at: new Date().toISOString(), fixtures: "testdata/landing/sessions.mjs", viewer: "Built slink local viewer; unmodified UI", branding: "Original Excerpt: brackets, two lines, and a dot", files: ["sessions", "focused", "focused-mobile", "prepare-view", "search", "agent-activity"].map(name => `${name}.webp`) };
  await writeFile(path.join(output, "capture.json"), JSON.stringify(manifest, null, 2) + "\n");
  console.log(`Product screenshots: ${output}`);
} finally {
  socket?.close(); browser?.kill("SIGTERM"); cli?.kill("SIGTERM");
}
