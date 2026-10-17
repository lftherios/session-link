#!/usr/bin/env node
// Capture the built product, using fictional sessions and an isolated local home.
// Run npm run build:viewer and rebuild slink first. See assets/product/README.md.
import assert from "node:assert/strict";
import { mkdir, mkdtemp, realpath, writeFile, utimes } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import os from "node:os";
import { sessions } from "../testdata/landing/sessions.mjs";
import { validateRun } from "../packages/format/validate.mjs";
import { launchBrowser, startViewer } from "./browser.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const output = path.resolve(process.env.SCREENSHOT_DIR ?? path.join(root, "assets/product"));
const SCALE = 2; // device scale factor; the landing page sizes images in CSS px
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

let cli, browser;
try {
  cli = await startViewer(["--pick"], { home: localHome, cwd: project, binary: process.env.SLINK_BINARY ?? "/tmp/session-link-landing-slink" });
  const { url: localURL, origin: base } = cli;
  browser = await launchBrowser({ profile: path.join(workspace, "browser"), installed: true });
  const page = await browser.firstPage(), errors = browser.errors;
  const { send, evaluate, waitFor, fill: field } = page;
  const click = selector => evaluate(`document.querySelector(${JSON.stringify(selector)}).click()`);
  const capture = async (name, width = 1280, height = 940) => {
    // 2x so the hero stays sharp on HiDPI displays; the page declares CSS sizes.
    await send("Emulation.setDeviceMetricsOverride", { width, height, deviceScaleFactor: SCALE, mobile: false });
    await evaluate("document.activeElement?.blur();document.fonts.ready.then(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))))");
    assert.equal(await evaluate("document.documentElement.scrollWidth > innerWidth"), false, `${name}: horizontal overflow`);
    await writeFile(path.join(output, `${name}.webp`), await page.screenshot({ format: "webp", quality: 88 }));
    console.log(`Captured ${name}.webp (${width} × ${height} CSS px at ${SCALE}x)`);
  };
  await send("Emulation.setTimezoneOverride", { timezoneId: "Europe/Berlin" });
  await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "light" }] });
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 940, deviceScaleFactor: SCALE, mobile: false });
  await send("Page.navigate", { url: base + new URL(localURL).hash });
  await waitFor("document.querySelectorAll('.row[data-source]').length===3");
  await capture("sessions", 1100, 680);
  await click('[data-source="0"]');
  await waitFor("!!document.querySelector('.sv-response-body table')");
  assert.match(await evaluate("document.querySelector('.sv-position').textContent"), /4 of 4/);
  await capture("focused-mobile", 390, 740);
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
  await send("Page.navigate", { url: base + new URL(localURL).hash });
  await waitFor("document.querySelectorAll('.row[data-source]').length===3");
  await click('[data-source="1"]');
  await waitFor("!!document.querySelector('.sv-response-body') && document.querySelector('.sv-position').textContent.includes('3 of 3')");
  await click(".sv-support>summary");
  await waitFor("document.querySelector('.sv-support').open");
  await capture("agent-activity", 1280, 1200);
  assert.deepEqual(errors, [], "browser runtime errors");
  const manifest = { captured_at: new Date().toISOString(), device_scale_factor: SCALE, fixtures: "testdata/landing/sessions.mjs", viewer: "Built slink local viewer; unmodified UI", branding: "Elision: brackets around three dots", files: ["sessions", "focused", "focused-mobile", "prepare-view", "search", "agent-activity"].map(name => `${name}.webp`) };
  await writeFile(path.join(output, "capture.json"), JSON.stringify(manifest, null, 2) + "\n");
  console.log(`Product screenshots: ${output}`);
} finally {
  browser?.close(); cli?.child.kill("SIGTERM");
}
