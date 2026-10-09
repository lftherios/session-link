#!/usr/bin/env node
/**
 * Record the landing page's "How it works" section as the animation the
 * README opens with: assets/product/how-it-works.gif.
 *
 *   node scripts/capture-how-it-works.mjs
 *
 * The landing page is in the server repository, so this serves that
 * checkout's public/ directory on loopback, opens it in a headless browser,
 * brings the section into view and photographs it while its three steps play
 * through once. Nothing is drawn or edited here: the frames are the page's
 * own, and scripts/gif.mjs only turns them into a GIF.
 *
 *   LANDING_DIR      the server checkout's public/ (default ../session-link-server/public)
 *   BROWSER_BINARY   a Chromium-family browser (default: Brave on macOS)
 *   SCREENSHOT_DIR   where the GIF goes (default assets/product)
 *   SCALE            device pixels per CSS pixel (default 1.5, sharp at the
 *                    README's width on a high-density screen)
 */
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { readFile, mkdtemp, rm, writeFile } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { decodePNG, encodeGIF, paletteBuilder } from "./gif.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const landing = path.resolve(root, process.env.LANDING_DIR ?? "../session-link-server/public");
const output = path.join(process.env.SCREENSHOT_DIR ?? path.join(root, "assets/product"), "how-it-works.gif");
const SCALE = Number(process.env.SCALE ?? 1.5);
// The section's layout with the three steps beside one terminal starts at
// 981px; at this width the widest terminal line fits without being cut.
const VIEW = { width: 1280, height: 900 };
// Room around the steps for the chosen step's shadow and the link under the terminal.
const MARGIN = { side: 28, top: 28, bottom: 20 };
const HOLD = 4000; // how long the last frame stays before the loop starts again
// A step changing fades most of the picture at once, and every frame of a
// fade costs as much as a new picture, where typing changes a few characters.
// A frame that changes more than MUCH of the picture, on the way to another
// that does, is kept only every FADE milliseconds. A fade keeps its last
// frame, and nothing that only types is dropped.
const MUCH = 0.02, FADE = 140;

const types = { ".html": "text/html", ".css": "text/css", ".svg": "image/svg+xml", ".png": "image/png", ".webp": "image/webp", ".js": "text/javascript", ".json": "application/json" };
const server = http.createServer(async (req, res) => {
  const url = new URL(req.url, "http://localhost").pathname;
  const file = path.join(landing, url === "/" ? "landing.html" : path.normalize(url));
  try {
    if (!file.startsWith(landing)) throw new Error("outside the directory");
    const body = await readFile(file);
    res.writeHead(200, { "content-type": types[path.extname(file)] ?? "application/octet-stream" }).end(body);
  } catch { res.writeHead(404).end(); }
});

const workspace = await mkdtemp(path.join(os.tmpdir(), "slink-how-it-works-"));
let browser, socket;
try {
  await readFile(path.join(landing, "landing.html")).catch(() => { throw new Error(`no landing.html in ${landing}; set LANDING_DIR to the server checkout's public directory`); });
  await new Promise(resolve => server.listen(0, "127.0.0.1", resolve));
  const base = `http://127.0.0.1:${server.address().port}`;

  browser = spawn(process.env.BROWSER_BINARY ?? "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser", [
    "--headless", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
    "--disable-background-networking", "--disable-component-update", "--disable-sync",
    "--remote-debugging-port=0", `--user-data-dir=${path.join(workspace, "browser")}`, "about:blank",
  ], { stdio: ["ignore", "ignore", "pipe"] });
  const endpoint = await new Promise((resolve, reject) => {
    let text = "";
    const timer = setTimeout(() => reject(new Error(`the browser did not start: ${text.slice(-400)}`)), 20000);
    browser.stderr.on("data", chunk => { text += chunk; const found = text.match(/ws:\/\/[^\s]+/); if (found) { clearTimeout(timer); resolve(found[0]); } });
    browser.on("error", reject);
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
  const evaluate = async expression => {
    const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true });
    if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
    return result.result.value;
  };

  await send("Page.enable");
  await send("Emulation.setDeviceMetricsOverride", { ...VIEW, deviceScaleFactor: SCALE, mobile: false });
  await send("Page.navigate", { url: `${base}/` });
  for (let i = 0; i < 200 && !(await evaluate("document.readyState === 'complete' && !!document.getElementById('steps')")); i++) await new Promise(resolve => setTimeout(resolve, 50));
  await evaluate("document.fonts.ready.then(() => true)");
  assert.equal(await evaluate("matchMedia('(prefers-reduced-motion: reduce)').matches"), false, "the browser asks for reduced motion, so the steps would not play");

  // The section rises into place the first time it is seen, and the steps
  // start then too. Put it in place first, so the recording opens on the
  // steps at rest, then bring them into view, which starts them.
  const clip = JSON.parse(await evaluate(`(() => {
    for (const el of document.querySelectorAll('#start [data-reveal]')) { el.classList.add('in'); el.removeAttribute('data-reveal'); }
    const steps = document.getElementById('steps');
    const top = steps.getBoundingClientRect().top + scrollY;
    scrollTo({ top: top - ${MARGIN.top} - 40, behavior: 'instant' });
    const box = steps.getBoundingClientRect();
    return JSON.stringify({ x: box.left - ${MARGIN.side}, y: box.top + scrollY - ${MARGIN.top}, width: box.width + ${2 * MARGIN.side}, height: box.height + ${MARGIN.top + MARGIN.bottom} });
  })()`));
  for (const key of Object.keys(clip)) clip[key] = Math.round(clip[key]);
  assert.ok(clip.x >= 0 && clip.width <= VIEW.width && clip.height <= VIEW.height, `the steps do not fit the view: ${JSON.stringify(clip)}`);

  // One photograph after another, as fast as the browser gives them, each
  // kept with the time it arrived. The steps are done when the last one's
  // link has appeared; a little longer lets it settle.
  const state = "JSON.stringify({ active: [...document.querySelectorAll('.step')].findIndex(s => s.classList.contains('is-active')), done: !!document.querySelector('.step:last-child .pop.on') })";
  const shots = [], order = [];
  const started = performance.now();
  let doneAt = 0;
  while (true) {
    const { data } = await send("Page.captureScreenshot", { format: "png", optimizeForSpeed: true, clip: { ...clip, scale: 1 } });
    const now = performance.now();
    shots.push({ png: Buffer.from(data, "base64"), at: now });
    const { active, done } = JSON.parse(await evaluate(state));
    if (order.at(-1) !== active) order.push(active);
    if (done && !doneAt) doneAt = now;
    if (doneAt && now - doneAt > 1500) break;
    assert.ok(now - started < 90000, "the steps did not finish playing");
  }
  assert.deepEqual(order, [0, 1, 2], "the three steps play in order, once");
  assert.ok(await evaluate("[...document.querySelectorAll('.step .tl')].every(line => line.classList.contains('on'))"), "every terminal line was shown");

  // Two passes over the frames: the first to choose one palette for all of
  // them, the second to redraw each in it.
  const builder = paletteBuilder();
  let size;
  for (const shot of shots) { const image = decodePNG(shot.png); size ??= image; builder.add(image); }
  const palette = builder.build(255);
  const drawn = shots.map((shot, n) => ({ pixels: palette.map(decodePNG(shot.png)), delay: n + 1 < shots.length ? shots[n + 1].at - shot.at : HOLD }));
  const changed = (a, b) => { let count = 0; for (let p = 0; p < a.length; p++) if (a[p] !== b[p]) count++; return count / a.length; };
  const frames = [drawn[0]];
  let sinceKept = 0;
  for (let n = 1; n < drawn.length; n++) {
    const last = frames.at(-1);
    sinceKept += drawn[n - 1].delay;
    const fading = changed(drawn[n].pixels, last.pixels) > MUCH && n + 1 < drawn.length && changed(drawn[n + 1].pixels, drawn[n].pixels) > MUCH;
    if (fading && sinceKept < FADE) last.delay += drawn[n].delay;
    else { frames.push(drawn[n]); sinceKept = 0; }
  }
  const gif = encodeGIF({ width: size.width, height: size.height, colors: palette.colors, frames });
  await writeFile(output, gif.bytes);
  const seconds = frames.reduce((total, frame) => total + frame.delay, 0) / 1000;
  console.log(`Recorded ${path.relative(root, output)}: ${size.width} × ${size.height}, ${gif.frames} frames from ${shots.length} photographs, ${seconds.toFixed(1)} s, ${(gif.bytes.length / 1024).toFixed(0)} KB`);
} finally {
  socket?.close();
  browser?.kill();
  server.close();
  await rm(workspace, { recursive: true, force: true });
}
