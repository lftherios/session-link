#!/usr/bin/env node
// Export assets/social-preview.png from assets/social-preview.svg: the SVG as a
// Chromium-family browser draws it, at 1280 × 640. See assets/brand/README.md.
import { spawn } from "node:child_process";
import { mkdtemp, writeFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";
import os from "node:os";

const root = fileURLToPath(new URL("../", import.meta.url));
const source = path.join(root, "assets/social-preview.svg");
const output = path.join(root, "assets/social-preview.png");
const profile = await mkdtemp(path.join(os.tmpdir(), "slink-social-"));

const browser = spawn(process.env.BROWSER_BINARY ?? "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser", [
  "--headless", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
  "--disable-background-networking", "--disable-component-update", "--disable-sync",
  "--remote-debugging-port=0", `--user-data-dir=${profile}`, "about:blank",
], { stdio: ["ignore", "ignore", "pipe"] });
let socket;
try {
  const endpoint = await new Promise((resolve, reject) => {
    let output = "";
    const timer = setTimeout(() => reject(new Error(`Browser startup timed out: ${output.slice(-1000)}`)), 15000);
    browser.stderr.on("data", chunk => {
      output += chunk;
      const match = output.match(/ws:\/\/[^\s]+/);
      if (match) { clearTimeout(timer); resolve(match[0]); }
    });
    browser.once("error", error => { clearTimeout(timer); reject(error); });
  });
  // The first page can appear a moment after the debugging endpoint does.
  let pageTarget;
  for (let i = 0; i < 100 && !pageTarget; i++) {
    pageTarget = (await (await fetch(`http://127.0.0.1:${new URL(endpoint).port}/json/list`)).json()).find(item => item.type === "page");
    if (!pageTarget) await new Promise(resolve => setTimeout(resolve, 50));
  }
  socket = new WebSocket(pageTarget.webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let sequence = 0;
  const pending = new Map();
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (!message.id) return;
    const call = pending.get(message.id); pending.delete(message.id);
    message.error ? call.reject(new Error(JSON.stringify(message.error))) : call.resolve(message.result);
  };
  const send = (method, params = {}) => new Promise((resolve, reject) => {
    const id = ++sequence; pending.set(id, { resolve, reject }); socket.send(JSON.stringify({ id, method, params }));
  });
  await send("Page.enable"); await send("Runtime.enable");
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 640, deviceScaleFactor: 1, mobile: false });
  await send("Page.navigate", { url: pathToFileURL(source).href });
  for (let i = 0; i < 200; i++) {
    const ready = await send("Runtime.evaluate", { expression: "document.readyState==='complete' && !!document.querySelector('svg text')", returnByValue: true });
    if (ready.result.value) break;
    await new Promise(resolve => setTimeout(resolve, 50));
  }
  await send("Runtime.evaluate", { expression: "document.fonts.ready.then(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))))", awaitPromise: true });
  const { data } = await send("Page.captureScreenshot", { format: "png" });
  await writeFile(output, Buffer.from(data, "base64"));
  console.log(`Captured ${path.relative(root, output)} (1280 × 640)`);
} finally {
  socket?.close(); browser.kill("SIGTERM");
}
