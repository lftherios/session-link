import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readdir, readFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { writeRun } from "../cli/store.mjs";

test("concurrent writeRun calls to one capture never collide or leak tmp files", async () => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "slink-store-"));
  const file = path.join(dir, "cap.json");
  try {
    // Pre-fix, all writers shared `${file}.tmp` and racing renames threw
    // ENOENT — the crash at the end of `slink dev -- <fast command>`.
    await Promise.all(
      Array.from({ length: 25 }, (_, i) => writeRun(file, { schema: "session/v0", i })),
    );
    const parsed = JSON.parse(await readFile(file, "utf8"));
    assert.equal(parsed.schema, "session/v0");
    const leftovers = (await readdir(dir)).filter((f) => f.includes(".tmp"));
    assert.deepEqual(leftovers, []);
  } finally {
    await rm(dir, { recursive: true, force: true });
  }
});
