import assert from "node:assert/strict";
import { test } from "node:test";
import { mkdtemp, readFile, writeFile, rm, stat } from "node:fs/promises";
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
  // A session with a transcript on disk and a turn captured live.
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
  if (process.platform !== "win32") {
    assert.equal((await stat(published[2])).mode & 0o777, 0o600);
    assert.equal((await stat(path.dirname(published[2]))).mode & 0o777, 0o700);
  }
  assert.match(notices.at(-1).text, /published.*https:\/\/example\.test/);

  await invoke("typo", ctx);
  assert.equal(notices.at(-1).level, "error");
  assert.deepEqual(JSON.parse(await readFile(log, "utf8")), published, "an unknown action must never publish");

  await writeFile(bridge, "console.error('The selected session cannot be read'); process.exitCode=1;");
  await invoke("view", ctx);
  assert.match(notices.at(-1).text, /preview failed.*selected session cannot be read/);
});

async function loadExtension(t, cliSource) {
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
${cliSource}`);
  process.env.SLINK_BIN = `${process.execPath} ${bridge}`;
  process.env.SLINK_HOME = dir;
  const compiled = path.join(dir, "extension.mjs");
  await build({ entryPoints: ["packages/pi-extension/index.ts"], outfile: compiled, bundle: true, platform: "node", format: "esm", logLevel: "silent" });
  const register = (await import(pathToFileURL(compiled).href)).default;
  const events = new Map(), commands = new Map(), notices = [];
  register({ on: (name, handler) => events.set(name, handler), registerCommand: (name, command) => commands.set(name, command) });
  const liveTurn = () => {
    events.get("before_agent_start")({ systemPrompt: "help", prompt: "Continue the research" });
    events.get("before_provider_request")({ payload: { model: "test" } });
    events.get("turn_end")({ message: { role: "assistant", content: [{ type: "text", text: "New finding" }], model: "test", provider: "test" }, toolResults: [] });
  };
  return { dir, log, events, notices, liveTurn, slink: commands.get("slink").handler, args: async () => JSON.parse(await readFile(log, "utf8")) };
}

test("pi /slink publishes the whole session: the live capture, or the transcript once resumed", async (t) => {
  const ext = await loadExtension(t, "console.log('https://example.test/s/published');");
  const sessionFile = path.join(ext.dir, "session.jsonl");
  const ctx = (earlier) => ({ cwd: "/work/project", ui: { notify: (text, level) => ext.notices.push({ text, level }) },
    sessionManager: { getSessionFile: () => sessionFile, getSessionId: () => "session-1", getSessionName: () => "Research", getBranch: () => earlier } });

  // Started here: every turn is in the live capture, which is exact.
  const fresh = ctx([]);
  ext.events.get("session_start")({ reason: "startup" }, fresh);
  ext.liveTurn();
  await ext.slink("", fresh);
  assert.deepEqual((await ext.args()).slice(0, 2), ["push", "--yes"]);

  // Resumed: the session already had turns this run never saw.
  const resumed = ctx([{ type: "model_change" }, { type: "message", message: { role: "user" } }, { type: "message", message: { role: "assistant" } }]);
  ext.events.get("session_start")({ reason: "resume" }, resumed);
  ext.liveTurn();
  await ext.slink("", resumed);
  assert.deepEqual(await ext.args(), ["share", "--from", "pi", "--session", sessionFile, "--yes"]);
  assert.match(ext.notices.at(-1).text, /published.*https:\/\/example\.test/);

  // Without a transcript on disk the live capture is all there is.
  const unsaved = { ...resumed, sessionManager: { ...resumed.sessionManager, getSessionFile: () => undefined } };
  ext.events.get("session_start")({ reason: "resume" }, unsaved);
  ext.liveTurn();
  await ext.slink("", unsaved);
  assert.deepEqual((await ext.args()).slice(0, 2), ["push", "--yes"]);
});

test("pi /slink reports why a publish failed, not the last line slink printed", async (t) => {
  // What `slink push` prints when its gate finds a credential.
  const blocked = ['"Research"', "  2 spans · test/test · 1.2KB · exact", "✗ publish blocked — the session appears to contain credentials:", "    github-token  ghp_…", "  redact the local file and push again:", "    /home/dev/.slink/runs/20261001-100000-abc123.json"];
  const ext = await loadExtension(t, `console.error(${JSON.stringify(blocked.join("\n"))}); process.exitCode = 1;`);
  const ctx = { cwd: "/work/project", ui: { notify: (text, level) => ext.notices.push({ text, level }) },
    sessionManager: { getSessionFile: () => undefined, getSessionId: () => "session-1", getSessionName: () => "Research" } };
  ext.events.get("session_start")({ reason: "startup" }, ctx);
  ext.liveTurn();
  await ext.slink("", ctx);
  assert.equal(ext.notices.at(-1).level, "error");
  assert.equal(ext.notices.at(-1).text, "session.link: publish failed — publish blocked — the session appears to contain credentials");
});
