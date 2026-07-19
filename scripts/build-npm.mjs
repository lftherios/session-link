#!/usr/bin/env node
// Build the npm binary channel for the Go slink CLI (P4). Generates the
// per-platform packages (@session-link/cli-<goos>-<goarch>, each just the
// binary + an os/cpu-gated package.json) and the `session.link` launcher
// (optionalDependencies on all of them + the resolve-and-exec bin shim),
// so `npx session.link` keeps working after the migration.
//
//   node scripts/build-npm.mjs <version>            # build into go/dist-npm
//   node scripts/build-npm.mjs <version> --publish  # ...then npm publish
//
// The launcher is published LAST so its optionalDependencies already
// resolve; a matching version pins them exactly. Cross-compilation is Go's
// own (CGO off → static), so this path doesn't depend on goreleaser.
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, rmSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const goDir = path.join(root, "go");
const outDir = path.join(goDir, "dist-npm");

const version = process.argv[2];
const publish = process.argv.includes("--publish");
if (!version || version.startsWith("-")) {
  console.error("usage: node scripts/build-npm.mjs <version> [--publish]");
  process.exit(1);
}

// node os/cpu values, keyed by go os/arch.
const NODE_OS = { darwin: "darwin", linux: "linux", windows: "win32" };
const NODE_CPU = { amd64: "x64", arm64: "arm64" };
const TARGETS = [
  ["darwin", "amd64"],
  ["darwin", "arm64"],
  ["linux", "amd64"],
  ["linux", "arm64"],
  ["windows", "amd64"],
  ["windows", "arm64"],
];

rmSync(outDir, { recursive: true, force: true });
mkdirSync(outDir, { recursive: true });

const optionalDependencies = {};
for (const [goos, goarch] of TARGETS) {
  const name = `@session-link/cli-${goos}-${goarch}`;
  const dir = path.join(outDir, `cli-${goos}-${goarch}`);
  mkdirSync(dir, { recursive: true });
  const exe = goos === "windows" ? "slink.exe" : "slink";
  execFileSync(
    "go",
    ["build", "-trimpath", "-ldflags", `-s -w -X main.version=${version}`, "-o", path.join(dir, exe), "./cmd/slink"],
    { cwd: goDir, env: { ...process.env, GOOS: goos, GOARCH: goarch, CGO_ENABLED: "0" }, stdio: "inherit" },
  );
  writeFileSync(
    path.join(dir, "package.json"),
    JSON.stringify(
      {
        name,
        version,
        description: `slink CLI binary for ${goos}/${goarch}`,
        homepage: "https://session.link",
        license: "MIT",
        os: [NODE_OS[goos]],
        cpu: [NODE_CPU[goarch]],
        files: [exe],
      },
      null,
      2,
    ) + "\n",
  );
  optionalDependencies[name] = version;
  console.log(`✓ ${name}`);
}

// The launcher — the `session.link` name, so the npx funnel is unchanged.
const launcherDir = path.join(outDir, "session.link");
mkdirSync(path.join(launcherDir, "bin"), { recursive: true });
copyFileSync(path.join(root, "packaging", "npm", "bin.js"), path.join(launcherDir, "bin", "slink.js"));
copyFileSync(path.join(root, "README.md"), path.join(launcherDir, "README.md"));
writeFileSync(
  path.join(launcherDir, "package.json"),
  JSON.stringify(
    {
      name: "session.link",
      version,
      description: "Capture LLM sessions locally, publish the ones worth sharing",
      homepage: "https://session.link",
      license: "MIT",
      bin: { slink: "bin/slink.js" },
      // Non-matching platforms fail to install silently — the point of the
      // pattern; the launcher resolves whichever one did install.
      optionalDependencies,
      files: ["bin"],
    },
    null,
    2,
  ) + "\n",
);
console.log("✓ session.link launcher");

if (publish) {
  // Platform packages first, launcher last (so its optional deps resolve).
  for (const [goos, goarch] of TARGETS) {
    execFileSync("npm", ["publish", "--access", "public"], {
      cwd: path.join(outDir, `cli-${goos}-${goarch}`),
      stdio: "inherit",
    });
  }
  execFileSync("npm", ["publish", "--access", "public"], { cwd: launcherDir, stdio: "inherit" });
  console.log("✓ published the npm binary channel");
}
