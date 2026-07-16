#!/usr/bin/env node
// Bundle the slink CLI into one self-contained file for distribution:
// packages/cli/dist/slink.mjs, with ajv + the session/v0 schema + the viewer
// (as a string module) all inlined, so the published package has zero runtime
// deps and `slink open` needs no build. The same entry feeds `bun compile`.
import { chmod, mkdir } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { spawnSync } from "node:child_process";
import esbuild from "esbuild";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const outfile = path.join(root, "packages", "cli", "dist", "slink.mjs");

// The viewer must exist as a string module before we bundle (open.mjs imports it).
const vb = spawnSync("node", [path.join(root, "scripts", "build-viewer.mjs")], {
  stdio: "inherit",
});
if (vb.status !== 0) process.exit(vb.status ?? 1);

await mkdir(path.dirname(outfile), { recursive: true });
await esbuild.build({
  entryPoints: [path.join(root, "cli", "slink.mjs")],
  outfile,
  bundle: true,
  platform: "node",
  format: "esm",
  target: "node20",
  // Everything inlined (ajv, ajv-formats, the schema JSON, the viewer string)
  // → a zero-dependency package and a portable single file for bun compile.
  external: [],
  // No banner shebang: esbuild hoists the entry's own #! to line 1; a banner
  // would add a second, invalid mid-file shebang.
  loader: { ".json": "json" },
  logLevel: "warning",
});
await chmod(outfile, 0o755);
console.log(`built ${path.relative(process.cwd(), outfile)}`);
