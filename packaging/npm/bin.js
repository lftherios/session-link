#!/usr/bin/env node
// The `session.link` launcher: resolve the platform-matched slink binary
// (installed as an optionalDependency) and exec it. This keeps
// `npx session.link` / `npm i -g session.link` working with the Go build —
// npm only downloads the one @session-link/cli-<os>-<arch> package that
// matches, via that package's os/cpu fields.
"use strict";

const { spawnSync } = require("node:child_process");

// node platform/arch → the goos/goarch used in the package names.
const GOOS = { darwin: "darwin", linux: "linux", win32: "windows" };
const GOARCH = { x64: "amd64", arm64: "arm64" };

function resolveBinary() {
  const goos = GOOS[process.platform];
  const goarch = GOARCH[process.arch];
  if (!goos || !goarch) {
    throw new Error(`unsupported platform ${process.platform}/${process.arch}`);
  }
  const pkg = `@session-link/cli-${goos}-${goarch}`;
  const exe = process.platform === "win32" ? "slink.exe" : "slink";
  try {
    // Resolves the binary file inside the platform package.
    return require.resolve(`${pkg}/${exe}`);
  } catch {
    throw new Error(
      `${pkg} isn't installed — reinstall session.link (npm sometimes skips ` +
        `optional deps; try 'npm i -g session.link --force'), or install via ` +
        `brew/curl: https://session.link/install.sh`,
    );
  }
}

let binary;
try {
  binary = resolveBinary();
} catch (err) {
  console.error(`session.link: ${err.message}`);
  process.exit(1);
}

const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });
if (result.error) {
  console.error(`session.link: ${result.error.message}`);
  process.exit(1);
}
// Forward the child's exit status (or its terminating signal).
if (result.signal) process.kill(process.pid, result.signal);
process.exit(result.status === null ? 1 : result.status);
