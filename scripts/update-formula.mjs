#!/usr/bin/env node
// Fill the Homebrew formula's version + sha256 values from a release's
// SHA256SUMS.txt — the manual chore that otherwise makes each release
// error-prone. Run in the tap repo (or here) after a release:
//
//   node scripts/update-formula.mjs <version> [SHA256SUMS.txt] [formula.rb]
//
// SHA256SUMS.txt is the `sha256sum *` output the release workflow attaches.
import { readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const [version, sumsPath, formulaPath] = [
  process.argv[2],
  process.argv[3] ?? path.join(root, "packages", "cli", "binaries", "SHA256SUMS.txt"),
  process.argv[4] ?? path.join(root, "packaging", "homebrew-tap", "Formula", "slink.rb"),
];
if (!version) {
  console.error("usage: node scripts/update-formula.mjs <version> [SHA256SUMS.txt] [formula.rb]");
  process.exit(1);
}

// asset name → sha256 (SHA256SUMS.txt lines are "<hash>  <asset>").
const sums = new Map();
for (const line of (await readFile(sumsPath, "utf8")).split("\n")) {
  const m = /^([0-9a-f]{64})\s+\*?(.+)$/.exec(line.trim());
  if (m) sums.set(m[2], m[1]);
}

// Homebrew covers macOS + Linux (no Windows). Anchor each replacement on the
// url's asset name — stable across placeholder and already-filled states, so
// re-running on a filled formula is idempotent and never crosses slots.
const ASSETS = ["slink-darwin-arm64", "slink-darwin-x64", "slink-linux-arm64", "slink-linux-x64"];

let formula = await readFile(formulaPath, "utf8");
formula = formula.replace(/version "[^"]*"/, `version "${version.replace(/^v/, "")}"`);
for (const asset of ASSETS) {
  const sha = sums.get(asset);
  if (!sha) {
    console.error(`error: ${asset} missing from ${sumsPath}`);
    process.exit(1);
  }
  // (url "...<asset>"  \n  sha256 ")<value>(")
  const re = new RegExp(`(url "[^"]*${asset}"\\s*\\n\\s*sha256 ")[^"]*(")`);
  if (!re.test(formula)) {
    console.error(`error: no url/sha256 block for ${asset} in the formula`);
    process.exit(1);
  }
  formula = formula.replace(re, `$1${sha}$2`);
}
await writeFile(formulaPath, formula);
console.log(`updated ${path.relative(process.cwd(), formulaPath)} → v${version.replace(/^v/, "")}`);
