#!/usr/bin/env node
// Export assets/social-preview.png from assets/social-preview.svg: the SVG as a
// Chromium-family browser draws it, at 1280 × 640. See assets/brand/README.md.
import { mkdtemp, writeFile } from "node:fs/promises";
import { fileURLToPath, pathToFileURL } from "node:url";
import path from "node:path";
import os from "node:os";
import { launchBrowser } from "./browser.mjs";

const root = fileURLToPath(new URL("../", import.meta.url));
const source = path.join(root, "assets/social-preview.svg");
const output = path.join(root, "assets/social-preview.png");
const profile = await mkdtemp(path.join(os.tmpdir(), "slink-social-"));

const browser = await launchBrowser({ profile, installed: true });
try {
  const page = await browser.firstPage();
  await page.send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 640, deviceScaleFactor: 1, mobile: false });
  await page.navigate(pathToFileURL(source).href);
  await page.waitFor("document.readyState==='complete' && !!document.querySelector('svg text')");
  await page.evaluate("document.fonts.ready.then(()=>new Promise(r=>requestAnimationFrame(()=>requestAnimationFrame(r))))");
  await writeFile(output, await page.screenshot());
  console.log(`Captured ${path.relative(root, output)} (1280 × 640)`);
} finally {
  browser.close();
}
