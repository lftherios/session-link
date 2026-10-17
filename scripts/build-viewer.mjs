#!/usr/bin/env node
// Bundle RunViewer into build/viewer/, the artifact both native builds embed
// and serve for `slink view`: the IIFE bundle, the brand files and a manifest
// with their digests. The viewer is React, so it ships as this built bundle
// inside the binary rather than being rewritten. (react + lucide + inline
// styles only.)
//
// The Go embed (go/internal/open/viewer.js and branding/) is copied from that
// artifact; the Rust build reads build/viewer/ directly. All of it is
// gitignored; CI rebuilds it before `go build` or `cargo build`.
import { execFileSync } from "node:child_process";
import { createHash } from "node:crypto";
import { fileURLToPath } from "node:url";
import { copyFile, mkdir, readFile, writeFile } from "node:fs/promises";
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
  define: { "process.env.NODE_ENV": '"production"' },
  logLevel: "warning",

});

const js = result.outputFiles[0].text;
const hostedArg = process.argv.indexOf("--hosted-output");
if (hostedArg !== -1) {
  const hosted = process.argv[hostedArg + 1];
  if (!hosted) throw new Error("--hosted-output requires a file path");
  await mkdir(path.dirname(hosted), { recursive: true });
  await writeFile(hosted, js);
}

const artifact = path.join(root, "build", "viewer");
const brand = ["brand.css", "favicon.svg", "mark.svg"];
await mkdir(artifact, { recursive: true });
await writeFile(path.join(artifact, "viewer.js"), js);
await Promise.all(brand.map(name => copyFile(path.join(root, "assets", "brand", name), path.join(artifact, name))));

// Where the artifact came from. dirty says the revision does not describe it:
// something the bundle is built from differs from that commit.
const inputs = ["packages/viewer", "packages/format", "assets/brand", "scripts/build-viewer.mjs", "scripts/viewer-validator.mjs", "package-lock.json"];
function source() {
  try {
    const git = (...args) => execFileSync("git", ["-C", root, ...args], { encoding: "utf8", stdio: ["ignore", "pipe", "ignore"] }).trim();
    return { revision: git("rev-parse", "HEAD"), dirty: git("status", "--porcelain", "--", ...inputs) !== "" };
  } catch {
    return { revision: null, dirty: null };
  }
}
const version = async (name) => JSON.parse(await readFile(path.join(root, "packages", name, "package.json"), "utf8")).version;
const files = [];
for (const name of [...brand, "viewer.js"]) {
  const bytes = await readFile(path.join(artifact, name));
  files.push({ path: name, bytes: bytes.length, sha256: createHash("sha256").update(bytes).digest("hex") });
}
const manifest = {
  schema: "slink-viewer-artifact/v1",
  ...source(),
  packages: { "@session-link/viewer": await version("viewer"), "@session-link/format": await version("format") },
  files,
};
await writeFile(path.join(artifact, "manifest.json"), JSON.stringify(manifest, null, 2) + "\n");

const embed = path.join(root, "go", "internal", "open");
await mkdir(path.join(embed, "branding"), { recursive: true });
await copyFile(path.join(artifact, "viewer.js"), path.join(embed, "viewer.js"));
await Promise.all(brand.map(name => copyFile(path.join(artifact, name), path.join(embed, "branding", name))));
console.log(`built build/viewer and go/internal/open/viewer.js (${(js.length / 1024).toFixed(0)} KB viewer)`);
