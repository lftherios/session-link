#!/usr/bin/env node
// Bundle RunViewer into go/internal/open/viewer.js — the IIFE the Go CLI
// go:embeds and serves for `slink open`. The viewer is React, so it ships
// as this built bundle inside the Go binary rather than being rewritten.
// (react + lucide + inline styles only.) Gitignored; CI rebuilds it before
// `go build`.
import { fileURLToPath } from "node:url";
import { copyFile, mkdir, writeFile } from "node:fs/promises";
import path from "node:path";
import esbuild from "esbuild";
import { buildValidator } from "./viewer-validator.mjs";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
await buildValidator();

const result = await esbuild.build({
  entryPoints: [path.join(root, "packages", "viewer", "viewer-entry.tsx")],
  bundle: true,
  minify: true,
  write: false,
  format: "iife",
  jsx: "automatic",
  alias: { "@": root },
  define: { "process.env.NODE_ENV": '"production"' },
  logLevel: "warning",

});

const js = result.outputFiles[0].text;
const out = path.join(root, "go", "internal", "open", "viewer.js");
const hostedArg = process.argv.indexOf("--hosted-output");
if (hostedArg !== -1) {
  const hosted = process.argv[hostedArg + 1];
  if (!hosted) throw new Error("--hosted-output requires a file path");
  await mkdir(path.dirname(hosted), { recursive: true });
  await writeFile(hosted, js);
}
await mkdir(path.dirname(out), { recursive: true });
await writeFile(out, js);
const branding = path.join(root, "go", "internal", "open", "branding");
await mkdir(branding, { recursive: true });
await Promise.all(["mark.svg", "favicon.svg", "brand.css"].map(name =>
  copyFile(path.join(root, "assets", "brand", name), path.join(branding, name))
));
console.log(`built go/internal/open/viewer.js (${(js.length / 1024).toFixed(0)} KB viewer)`);
