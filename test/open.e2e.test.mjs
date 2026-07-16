import assert from "node:assert/strict";
import { test } from "node:test";
import { spawn } from "node:child_process";
import { mkdtemp, rename, rm } from "node:fs/promises";
import { existsSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const ROOT = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const VIEWER = path.join(ROOT, "cli", "viewer.mjs");

function runSlink(args, env) {
  const proc = spawn(process.execPath, [path.join(ROOT, "cli/slink.mjs"), ...args], {
    cwd: ROOT,
    timeout: 15_000,
    env: { ...process.env, ...env },
  });
  let stderr = "";
  proc.stderr.on("data", (d) => (stderr += d));
  return new Promise((resolve) => proc.on("close", (code) => resolve({ code, stderr })));
}

// The viewer is a gitignored build artifact. `open` is the ONLY command that
// needs it — every other command (and CI `npm test`, which spawns the CLI for
// capture/import/login) must run without it. This test pins that boundary:
// a regression that eagerly imports the viewer would crash `help`/`list` here.
test("non-open commands run without the built viewer bundle", async () => {
  const home = await mkdtemp(path.join(os.tmpdir(), "slink-open-"));
  const hidden = existsSync(VIEWER) ? `${VIEWER}.hidden` : null;
  if (hidden) await rename(VIEWER, hidden);
  try {
    for (const cmd of [["help"], ["list"]]) {
      const { code, stderr } = await runSlink(cmd, { SLINK_HOME: home });
      assert.equal(code, 0, `slink ${cmd[0]} crashed without viewer: ${stderr}`);
      assert.doesNotMatch(stderr, /ERR_MODULE_NOT_FOUND|viewer\.mjs/, `slink ${cmd[0]} leaked a viewer import`);
    }
    // `open` needs it: a clean error, not a module-load stack trace.
    const { code, stderr } = await runSlink(["open", "--no-browser", "--port", "4799"], { SLINK_HOME: home });
    assert.equal(code, 1);
    assert.match(stderr, /viewer bundle not built/);
    assert.doesNotMatch(stderr, /ERR_MODULE_NOT_FOUND/, "should be a clean message, not a raw crash");
  } finally {
    if (hidden) await rename(hidden, VIEWER);
    await rm(home, { recursive: true, force: true });
  }
});
