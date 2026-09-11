import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readFile, writeFile, rm } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { pathToFileURL } from "node:url";
import { build } from "esbuild";

test("pi command pins resumed sessions, returns a local URL, and preserves explicit publishing", async (t) => {
  const dir = await mkdtemp(path.join(os.tmpdir(), "slink-pi-handoff-"));
  const previous = { SLINK_BIN: process.env.SLINK_BIN, SLINK_HOME: process.env.SLINK_HOME };
  t.after(async () => {
    for (const [key, value] of Object.entries(previous)) {
      if (value === undefined) delete process.env[key]; else process.env[key] = value;
    }
    await rm(dir, { recursive: true, force: true });
  });
  const bridge = path.join(dir, "cli.mjs"), log = path.join(dir, "args.json");
  await writeFile(bridge, `import {writeFileSync} from 'node:fs';
writeFileSync(${JSON.stringify(log)}, JSON.stringify(process.argv.slice(2)));
console.log(process.argv[2]==='view'?'http://127.0.0.1:4400/p/saved':'https://example.test/r/published');`);
  process.env.SLINK_BIN = `${process.execPath} ${bridge}`;
  process.env.SLINK_HOME = dir;
  const compiled = path.join(dir, "extension.mjs");
  await build({ entryPoints: ["packages/pi-extension/index.ts"], outfile: compiled, bundle: true, platform: "node", format: "esm", logLevel: "silent" });
  const register = (await import(pathToFileURL(compiled).href)).default;
  const events = new Map(), commands = new Map(), notices = [];
  register({ on: (name, handler) => events.set(name, handler), registerCommand: (name, command) => commands.set(name, command) });
  let sessionFile = path.join(dir, "resumed session.jsonl");
  const ctx = { cwd: "/work/project", ui: { notify: (text, level) => notices.push({ text, level }) },
    sessionManager: { getSessionFile: () => sessionFile, getSessionId: () => "session-1", getSessionName: () => "Research" } };
  const invoke = commands.get("slink").handler;
  events.get("session_start")({}, ctx);
  // Resumed work has historical messages on disk and a new live turn.
  events.get("before_agent_start")({ systemPrompt: "help", prompt: "Continue the research" });
  events.get("before_provider_request")({ payload: { model: "test" } });
  events.get("turn_end")({ message: { role: "assistant", content: [{ type: "text", text: "New finding" }], model: "test", provider: "test" }, toolResults: [] });
  await invoke("view", ctx);
  assert.deepEqual(JSON.parse(await readFile(log, "utf8")), ["view", "--from", "pi", "--session", sessionFile, "--background"]);
  assert.match(notices.at(-1).text, /local preview.*http:\/\/127\.0\.0\.1.*nothing uploaded/);

  await invoke("", ctx);
  const published = JSON.parse(await readFile(log, "utf8"));
  assert.equal(published[0], "push");
  assert.equal(published[1], "--yes");
  assert.match(published[2], /runs[/\\].+\.json$/);
  assert.match(notices.at(-1).text, /published.*https:\/\/example\.test/);

  await invoke("typo", ctx);
  assert.equal(notices.at(-1).level, "error");
  assert.deepEqual(JSON.parse(await readFile(log, "utf8")), published, "an unknown action must never publish");

  await writeFile(bridge, "console.error('The selected session cannot be read'); process.exitCode=1;");
  await invoke("view", ctx);
  assert.match(notices.at(-1).text, /preview failed.*selected session cannot be read/);
});
