#!/usr/bin/env node
// Compile the slink CLI to standalone per-platform binaries with `bun build
// --compile` — the Node-less distribution path (Homebrew, curl|sh, GitHub
// Releases). One entry (cli/slink.mjs) with the viewer string + schema + ajv
// all inlined; bun embeds them and the Bun runtime into each binary.
//
// usage: node scripts/build-binary.mjs [--host]   (--host builds only this machine's target)
import { spawnSync } from "node:child_process";
import { existsSync } from "node:fs";
import { fileURLToPath } from "node:url";
import path from "node:path";
import os from "node:os";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const entry = path.join(root, "cli", "slink.mjs");
const outDir = path.join(root, "packages", "cli", "binaries");
// BUN_BIN override, else the local installer path, else bun on PATH (CI).
const localBun = path.join(os.homedir(), ".bun", "bin", "bun");
const bun = process.env.BUN_BIN || (existsSync(localBun) ? localBun : "bun");

// bun target → the release asset name users download.
const TARGETS = [
  { target: "bun-darwin-arm64", asset: "slink-darwin-arm64" },
  { target: "bun-darwin-x64", asset: "slink-darwin-x64" },
  { target: "bun-linux-x64", asset: "slink-linux-x64" },
  { target: "bun-linux-arm64", asset: "slink-linux-arm64" },
  { target: "bun-windows-x64", asset: "slink-windows-x64.exe" },
];

function hostTarget() {
  const a = process.arch === "arm64" ? "arm64" : "x64";
  const p = process.platform === "darwin" ? "darwin" : process.platform === "win32" ? "windows" : "linux";
  return TARGETS.find((t) => t.target === `bun-${p}-${a}`) ?? TARGETS[0];
}

// The viewer string module must be fresh before we compile (open.mjs imports it).
const vb = spawnSync("node", [path.join(root, "scripts", "build-viewer.mjs")], { stdio: "inherit" });
if (vb.status !== 0) process.exit(vb.status ?? 1);

const targets = process.argv.includes("--host") ? [hostTarget()] : TARGETS;
for (const { target, asset } of targets) {
  const outfile = path.join(outDir, asset);
  console.log(`compiling ${target} → ${path.relative(process.cwd(), outfile)}`);
  const r = spawnSync(
    bun,
    ["build", entry, "--compile", `--target=${target}`, "--outfile", outfile, "--minify"],
    { stdio: "inherit" },
  );
  if (r.status !== 0) process.exit(r.status ?? 1);
}
console.log("done");
