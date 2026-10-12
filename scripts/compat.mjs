#!/usr/bin/env node
/**
 * Differential compatibility runner for the Rust migration
 * (docs/rust-migration.md, P0). It runs each case through two slink
 * executables, a reference and a candidate, each in its own throwaway home,
 * and compares what a user or a script can observe: the exit code, stdout,
 * stderr, and the files left in the slink home with their modes.
 *
 *   SLINK_BINARY=<go slink> node scripts/compat.mjs
 *   SLINK_BINARY=<go slink> SLINK_CANDIDATE=<rust slink> node scripts/compat.mjs
 *   node scripts/compat.mjs --list     the cases and how each is compared
 *   node scripts/compat.mjs <text>     only the cases whose name contains <text>
 *   node scripts/compat.mjs --keep …   leave the throwaway homes for reading
 *
 * Without SLINK_CANDIDATE the reference is compared with a second run of
 * itself. That checks the normalizations below cover everything that
 * legitimately differs between two runs, and it is what "the current binary
 * passes" means until there is a second implementation.
 *
 * Each observation is compared one of three ways:
 *   bytes     identical after normalization
 *   json      equal as parsed JSON after normalization; a case may name
 *             fields that hold a generated time, id or duration, and those
 *             are compared by presence and type only
 *   behavior  only what the case states: the exit code, which stream is
 *             used, and the patterns it names. For help text, whose spacing
 *             may differ, and for messages the flag parser writes.
 *
 * Normalization rewrites only values the runner knows were generated: the
 * throwaway root, the stamp and id of each capture created during the case,
 * loopback ports, and relative ages ("3d ago"), which move with the clock.
 * A difference that survives is a difference; widen a comparison in the case
 * that needs it, with the reason, never here.
 *
 * Nothing here reaches the network or the real ~/.slink: HOME and SLINK_HOME
 * point into the throwaway root, PATH is an empty directory (no clipboard, no
 * browser), and the only upstream is a replay server on loopback.
 */
import { spawn } from "node:child_process";
import { createHash } from "node:crypto";
import { existsSync, readFileSync, readdirSync } from "node:fs";
import { copyFile, mkdir, mkdtemp, readFile, readdir, realpath, rm, stat } from "node:fs/promises";
import http from "node:http";
import net from "node:net";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { isDeepStrictEqual } from "node:util";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const reference = process.env.SLINK_BINARY || "/tmp/session-link-slink";
const candidate = process.env.SLINK_CANDIDATE || reference;
const inventory = JSON.parse(readFileSync(path.join(root, "testdata", "compat", "cli.json"), "utf8"));

/* ------------------------------------------------------------------ cases */

const cases = [];
const add = (name, steps, extra = {}) => cases.push({ name, steps, ...extra });
const escape = (text) => text.replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
// A word of help text: the name with nothing word-like or dashed around it.
const word = (text) => new RegExp(`(^|[^\\w-])${escape(text)}([^\\w-]|$)`, "m");
const says = (...patterns) => ({ behavior: patterns });

// The top-level usage names every command; aliases appear beside theirs.
const usage = says(...inventory.commands.map((c) => word(c.name)), word("version"));
add("usage/no command", [{ args: [], exit: 1, stdout: "empty", stderr: usage }]);
for (const spelling of inventory.help) add(`usage/${spelling}`, [{ args: [spelling], exit: 0, stdout: usage, stderr: "empty" }]);
for (const spelling of inventory.version) {
  // The version is stamped at build time, so only its form is compared.
  add(`usage/${spelling}`, [{ args: [spelling], exit: 0, stdout: says(/^slink \S+\n$/), stderr: "empty" }]);
}
add("usage/unknown command, close to one", [{ args: ["shar"], exit: 1, stdout: "empty" }]);
add("usage/unknown command", [{ args: ["zzzzzz"], exit: 1, stdout: "empty" }]);

// Each command's own help goes to stdout, exits 0, and names its flags.
// record is the exception today: without "--" it hands its arguments to the
// flag package, which writes help to stderr. It is recorded as it behaves;
// whether the port keeps that is a decision listed in the baseline doc.
for (const command of inventory.commands) {
  const help = says(word(command.name), ...command.flags.map((flag) => word(`-${flag.name}`)));
  const streams = command.name === "record" ? { stdout: "empty", stderr: help } : { stdout: help, stderr: "empty" };
  for (const name of [command.name, ...command.aliases]) add(`help/${name} -h`, [{ args: [name, "-h"], exit: 0, ...streams }]);
  add(`help/help ${command.name}`, [{ args: ["help", command.name], exit: 0, ...streams }]);
}
add("flags/a value flag with no value", [{ args: ["list", "--limit"], exit: 2, stdout: "empty", stderr: says(/limit/) }]);
add("flags/an unknown flag", [{ args: ["list", "--nope"], exit: 2, stdout: "empty", stderr: says(/nope/) }]);

for (const shell of ["bash", "zsh", "fish"]) add(`completion/${shell}`, [{ args: ["completion", shell], exit: 0, stderr: "empty" }]);
add("completion/no shell", [{ args: ["completion"], exit: 2, stdout: "empty" }]);

add("shell/off in a shell that was never on", [{ args: ["off"], exit: 0 }]);
add("shell/on --no-start with no tap", [{ args: ["on", "--no-start", "--port", (box) => String(box.port)], exit: 1, stdout: "empty" }]);

add("local/an empty home", [
  { args: ["list"], exit: 0, stdout: "empty" },
  { args: ["ls"], exit: 0, stdout: "empty" },
  { args: ["list", "--json"], exit: 0, stdout: "json", stderr: "empty" },
  { args: ["prune"], exit: 0, stdout: "empty" },
  { args: ["push"], exit: 1, stdout: "empty" },
  { args: ["push", "no-such.json"], exit: 1, stdout: "empty" },
  { args: ["push", "nosuch"], exit: 1, stdout: "empty" },
  { args: ["import"], exit: 1, stdout: "empty" },
  { args: ["import", "--from", "claude-code"], exit: 1, stdout: "empty" },
  { args: ["import", "--from", "nope"], exit: 1, stdout: "empty" },
  { args: ["import", "no-such-transcript.jsonl"], exit: 1, stdout: "empty" },
]);

// Importing a transcript by path, for every fixture a path can name: each
// session of the fixture, the first one again (it must find the capture it
// already made), then the two listings. opencode and Hermes keep sessions in
// SQLite and their fixtures are rows, so they are not reached this way yet.
const fixtures = (...parts) => {
  const dir = path.join(root, "testdata", ...parts);
  return readdirSync(dir, { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => path.join(dir, entry.name)).sort();
};
function transcripts(harness, dir) {
  // A dsh fixture is a store as dsh lays it out, and can hold several sessions.
  if (harness === "dsh") {
    return readdirSync(dir, { recursive: true }).filter((file) => path.basename(file).startsWith("session.v4.jsonl")).sort().map((file) => path.join(dir, file));
  }
  const file = path.join(dir, harness === "aider" ? "input.chat.history.md" : "input.session.jsonl");
  return existsSync(file) ? [file] : [];
}
for (const harness of ["claude-code", "codex", "pi", "omp", "aider", "dsh"]) {
  const imports = (file, i = 0) => ({ args: ["import", "--from", harness, file], exit: 0, stdout: "empty", nextSecond: i > 0 });
  for (const dir of fixtures("import", harness)) {
    const files = transcripts(harness, dir);
    add(`import/${harness}/${path.basename(dir)}`, [
      // Captures list by file name, whose stamp has one-second resolution:
      // two made within a second list in the order of their random ids. So
      // each further import waits for the next second.
      ...files.map(imports),
      imports(files[0]),
      { args: ["list", "--json"], exit: 0, stdout: "json", stderr: "empty" },
      { args: ["list"], exit: 0 },
    ]);
  }
  // The agent is recognized from the transcript when --from is left out.
  const first = transcripts(harness, fixtures("import", harness)[0])[0];
  add(`import/${harness}/recognized without --from`, [{ args: ["import", first], exit: 0, stdout: "empty" }]);
}
const claude = path.join(root, "testdata", "import", "claude-code", "basic", "input.session.jsonl");
add("import/flags after the transcript", [{ args: ["import", claude, "--from", "claude-code"], exit: 0, stdout: "empty" }]);
add("import/--session names the transcript", [{ args: ["import", "--session", claude], exit: 0, stdout: "empty" }]);

// Discovery in a Claude Code store laid out as Claude Code lays it out: the
// newest session of the project, found from a subdirectory, by a pasted id,
// and the hint when the directory has none.
const session = "0c3b8f0e-5d0a-4b7e-9a51-3f6f2a1d7c44";
add(
  "discover/claude-code",
  [
    { cwd: "project/src/deep", args: ["import"], exit: 0, stdout: "empty" },
    { cwd: "project", args: ["import", "--from", "claude-code"], exit: 0, stdout: "empty" },
    { cwd: "project", args: ["import", session], exit: 0, stdout: "empty" },
    { cwd: "project", args: ["import", "no-such-session-id"], exit: 1, stdout: "empty" },
    { cwd: "elsewhere", args: ["import"], exit: 1, stdout: "empty" },
    { cwd: "elsewhere", args: ["share"], exit: 1, stdout: "empty" },
    { cwd: "project", args: ["share", "--yes"], exit: 1, stdout: "empty" },
  ],
  {
    async setup(box) {
      const project = path.join(box.work, "project");
      const store = path.join(box.home, ".claude", "projects", project.replace(/[^A-Za-z0-9]/g, "-"));
      await Promise.all([store, path.join(project, "src", "deep"), path.join(box.work, "elsewhere")].map((dir) => mkdir(dir, { recursive: true })));
      await copyFile(claude, path.join(store, `${session}.jsonl`));
    },
  },
);

// Recording: a child posts a recorded request through `slink record`, and a
// replay server answers with the recorded response. The child prints what it
// received, so stdout shows the response passed through untouched.
const child = `
const [file, kind] = process.argv.slice(1);
const url = kind.startsWith("anthropic") ? process.env.ANTHROPIC_BASE_URL + "/v1/messages"
  : process.env.OPENAI_BASE_URL + (kind === "openai.chat" ? "/chat/completions" : "/responses");
fetch(url, { method: "POST", headers: { "content-type": "application/json", authorization: "Bearer fictional" }, body: require("fs").readFileSync(file) })
  .then(async (r) => console.log(r.status, r.headers.get("content-type"), require("crypto").createHash("sha256").update(Buffer.from(await r.arrayBuffer())).digest("hex")));
`;
// The times in a recording are the recorder's own clock.
const recorded = { json: ["created_at", "started_at", "ended_at"] };
for (const dir of fixtures("proxy")) {
  const meta = JSON.parse(readFileSync(path.join(dir, "input.meta.json"), "utf8"));
  const upstream = (box) => `http://127.0.0.1:${box.upstream}`;
  add(
    `record/${path.basename(dir)}`,
    [
      {
        args: ["record", "--name", path.basename(dir), "--", process.execPath, "-e", child, path.join(dir, "input.request.json"), meta.kind],
        env: { SLINK_UPSTREAM_ANTHROPIC: upstream, SLINK_UPSTREAM_OPENAI: upstream },
        exit: 0,
      },
      { args: ["list", "--json"], exit: 0, stdout: { json: ["created_at"] }, stderr: "empty" },
    ],
    {
      files: [[/^runs\/<CAPTURE 1>\.json$/, recorded]],
      async setup(box) {
        const body = await readFile(path.join(dir, meta.streamed ? "input.response.sse" : "input.response.json"));
        const server = http.createServer((request, response) => {
          request.resume().on("end", () => {
            response.writeHead(meta.httpStatus, { "content-type": meta.streamed ? "text/event-stream" : "application/json" });
            response.end(body);
          });
        });
        await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
        box.upstream = server.address().port;
        box.cleanup.push(() => server.close());
      },
    },
  );
}
add("record/the child's exit code is slink's", [{ args: ["record", "--", process.execPath, "-e", "process.exit(7)"], exit: 7, stdout: "empty" }]);
add("record/a command that is not in PATH", [{ args: ["record", "--", "no-such-command"], exit: 127, stdout: "empty" }]);
add("record/a command that cannot start", [{ args: ["record", "--", "/no/such/command"], exit: 126, stdout: "empty" }]);
add("record/an argument before --", [{ args: ["record", "stray", "--", "true"], exit: 1, stdout: "empty" }]);

/* ----------------------------------------------------------------- engine */

const freePort = () =>
  new Promise((resolve, reject) => {
    const probe = net.createServer().once("error", reject);
    probe.listen(0, "127.0.0.1", () => {
      const { port } = probe.address();
      probe.close(() => resolve(port));
    });
  });

async function sandbox() {
  const dir = await realpath(await mkdtemp(path.join(os.tmpdir(), "slink-compat-")));
  const box = { root: dir, home: path.join(dir, "home"), work: path.join(dir, "work"), bin: path.join(dir, "bin"), port: await freePort(), captures: [], cleanup: [] };
  // The binary creates its own home, with its own modes: that is observed.
  box.slink = path.join(box.home, ".slink");
  await Promise.all([box.home, box.work, box.bin].map((d) => mkdir(d, { recursive: true })));
  return box;
}

function execute(binary, box, step) {
  const value = (v) => (typeof v === "function" ? v(box) : v);
  const env = { HOME: box.home, USERPROFILE: box.home, SLINK_HOME: box.slink, PATH: box.bin, TZ: "UTC", LANG: "C" };
  for (const [name, v] of Object.entries(step.env ?? {})) env[name] = value(v);
  return new Promise((resolve, reject) => {
    const proc = spawn(binary, step.args.map(value), { cwd: path.join(box.work, step.cwd ?? ""), env });
    const out = [];
    const err = [];
    proc.stdout.on("data", (chunk) => out.push(chunk));
    proc.stderr.on("data", (chunk) => err.push(chunk));
    const timer = setTimeout(() => proc.kill("SIGKILL"), 30_000);
    proc.on("error", reject);
    proc.on("close", (code, signal) => {
      clearTimeout(timer);
      resolve({ exit: signal ?? code, stdout: Buffer.concat(out).toString(), stderr: Buffer.concat(err).toString() });
    });
    proc.stdin.end(step.stdin ?? "");
  });
}

// Captures are named <stamp>-<id>.json, both generated. Number them in the
// order the steps created them, so the same capture has the same name on
// both sides: first the ones a step left on disk, then any it only named (a
// recording that captured nothing prints where it would have been saved).
async function noteCaptures(box, result) {
  const dir = path.join(box.slink, "runs");
  const known = (stamp, id) => box.captures.some((c) => c.stamp === stamp && c.id === id);
  const fresh = new Map();
  for (const name of await readdir(dir).catch(() => [])) {
    const found = name.match(/^(\d{8}-\d{6})-([0-9a-f]{6})\.json/);
    if (!found || known(found[1], found[2])) continue;
    const digest = await readFile(path.join(dir, `${found[1]}-${found[2]}.json`)).then((bytes) => createHash("sha256").update(bytes).digest("hex"), () => "");
    fresh.set(found[1] + found[2], { stamp: found[1], id: found[2], order: found[1] + digest });
  }
  box.captures.push(...[...fresh.values()].sort((a, b) => (a.order < b.order ? -1 : 1)));
  for (const [, stamp, id] of (result.stdout + result.stderr).matchAll(/runs[\\/](\d{8}-\d{6})-([0-9a-f]{6})\.json/g)) {
    if (!known(stamp, id)) box.captures.push({ stamp, id });
  }
}

function normalizer(box) {
  return (text, stream = false) => {
    let t = text.split(box.root).join("<ROOT>");
    box.captures.forEach((c, i) => {
      t = t.split(`${c.stamp}-${c.id}`).join(`<CAPTURE ${i + 1}>`);
      t = t.replace(new RegExp(`\\b${c.id}\\b`, "g"), `<ID ${i + 1}>`);
    });
    t = t.replace(/\b(127\.0\.0\.1|localhost):\d+/g, "$1:<PORT>").replace(new RegExp(`:${box.port}\\b`, "g"), ":<PORT>");
    // An age is padded to a column, so its padding moves with its width.
    if (stream) t = t.replace(/\b\d+[smhd] ago\b */g, "<AGE> ");
    return t;
  };
}

async function tree(dir, normalize, rel = "") {
  const entries = [];
  for (const entry of await readdir(path.join(dir, rel), { withFileTypes: true }).catch(() => [])) {
    const at = path.join(rel, entry.name);
    const mode = ((await stat(path.join(dir, at))).mode & 0o777).toString(8);
    const shown = normalize(at.split(path.sep).join("/"));
    if (entry.isDirectory()) entries.push({ path: shown + "/", mode }, ...(await tree(dir, normalize, at)));
    else entries.push({ path: shown, mode, text: normalize(await readFile(path.join(dir, at), "utf8")) });
  }
  return entries.sort((a, b) => (a.path < b.path ? -1 : 1));
}

async function observe(binary, test, keep) {
  const box = await sandbox();
  try {
    await test.setup?.(box);
    const raw = [];
    for (const step of test.steps) {
      if (step.nextSecond) await new Promise((resolve) => setTimeout(resolve, 1050 - (Date.now() % 1000)));
      raw.push(await execute(binary, box, step));
      await noteCaptures(box, raw.at(-1));
    }
    const normalize = normalizer(box);
    return {
      steps: raw.map((r) => ({ exit: r.exit, stdout: normalize(r.stdout, true), stderr: normalize(r.stderr, true) })),
      files: await tree(box.slink, normalize),
    };
  } finally {
    for (const close of box.cleanup) close();
    if (keep) console.log(`    kept ${box.root}`);
    else await rm(box.root, { recursive: true, force: true });
  }
}

const kind = (spec = "bytes") => (typeof spec === "string" ? spec : Object.keys(spec)[0]);
const clip = (line) => (line === undefined ? "(nothing)" : line.length > 160 ? line.slice(0, 159) + "…" : line);
function firstDifference(a, b) {
  const x = a.split("\n");
  const y = b.split("\n");
  let at = 0;
  while (at < x.length && x[at] === y[at]) at++;
  return `line ${at + 1}\n        reference: ${clip(x[at])}\n        candidate: ${clip(y[at])}`;
}
// A named field keeps its place and its type; only its value is set aside.
function withoutGenerated(value, fields) {
  if (Array.isArray(value)) return value.map((item) => withoutGenerated(item, fields));
  if (value === null || typeof value !== "object") return value;
  return Object.fromEntries(Object.entries(value).map(([key, v]) => [key, fields.includes(key) && (v === null || typeof v !== "object") ? `<${typeof v}>` : withoutGenerated(v, fields)]));
}

// The reason two observations of one thing do not agree, or null.
function disagreement(spec = "bytes", a, b) {
  switch (kind(spec)) {
    case "empty":
      return a === "" && b === "" ? null : `expected nothing\n        reference: ${clip(a.split("\n")[0])}\n        candidate: ${clip(b.split("\n")[0])}`;
    case "bytes":
      return a === b ? null : firstDifference(a, b);
    case "json": {
      const fields = spec.json ?? [];
      let x, y;
      try {
        x = withoutGenerated(JSON.parse(a), fields);
        y = withoutGenerated(JSON.parse(b), fields);
      } catch (error) {
        return `not JSON: ${error.message}`;
      }
      return isDeepStrictEqual(x, y) ? null : firstDifference(JSON.stringify(x, null, 1), JSON.stringify(y, null, 1));
    }
    case "behavior": {
      for (const [side, text] of [["reference", a], ["candidate", b]]) {
        if (text === "") return `the ${side} wrote nothing`;
        const missing = spec.behavior.find((pattern) => !pattern.test(text));
        if (missing) return `the ${side} does not match ${missing}`;
      }
      return null;
    }
  }
  throw new Error(`unknown comparison ${JSON.stringify(spec)}`);
}

function compare(test, a, b) {
  const problems = [];
  test.steps.forEach((step, i) => {
    const at = `step ${i + 1} (slink ${step.args.map((arg) => (typeof arg === "function" ? "…" : arg.includes("\n") ? "<script>" : arg)).join(" ")})`;
    for (const [side, seen] of [["reference", a.steps[i]], ["candidate", b.steps[i]]]) {
      if (seen.exit !== step.exit) problems.push(`${at}: the ${side} exited ${seen.exit}, the case expects ${step.exit}\n        stderr: ${clip(seen.stderr.split("\n")[0])}`);
    }
    for (const stream of ["stdout", "stderr"]) {
      const why = disagreement(step[stream], a.steps[i][stream], b.steps[i][stream]);
      if (why) problems.push(`${at}: ${stream} (${kind(step[stream])}): ${why}`);
    }
  });
  const listing = (side) => side.files.map((f) => `${f.mode} ${f.path}`).join("\n");
  if (listing(a) !== listing(b)) problems.push(`files and modes: ${firstDifference(listing(a), listing(b))}`);
  else {
    for (const [i, file] of a.files.entries()) {
      if (file.text === undefined) continue;
      const spec = (test.files ?? []).find(([pattern]) => pattern.test(file.path))?.[1];
      const why = disagreement(spec, file.text, b.files[i].text);
      if (why) problems.push(`${file.path} (${kind(spec)}): ${why}`);
    }
  }
  return problems;
}

/* -------------------------------------------------------------------- run */

const argv = process.argv.slice(2);
const keep = argv.includes("--keep");
const filter = argv.find((arg) => !arg.startsWith("--"));
const selected = cases.filter((test) => !filter || test.name.includes(filter));

if (argv.includes("--list")) {
  for (const test of selected) {
    const how = test.steps.map((step) => `${kind(step.stdout)}/${kind(step.stderr)}`).join(" ");
    console.log(`${test.name}\n    stdout/stderr per step: ${how}${test.files ? `\n    files: ${test.files.map(([pattern, spec]) => `${pattern} as ${kind(spec)}`).join(", ")}, the rest as bytes` : ""}`);
  }
  console.log(`\n${selected.length} cases`);
  process.exit(0);
}
for (const [role, binary] of [["reference", reference], ["candidate", candidate]]) {
  if (!existsSync(binary)) {
    console.error(`no ${role} binary at ${binary}\n  build one: cd go && go build -o /tmp/session-link-slink ./cmd/slink\n  or name it with ${role === "reference" ? "SLINK_BINARY" : "SLINK_CANDIDATE"}`);
    process.exit(1);
  }
}
console.log(`reference: ${reference}\ncandidate: ${candidate === reference ? "the reference again (a self-check of the normalizations)" : candidate}\n`);

let failed = 0;
for (const test of selected) {
  const [a, b] = await Promise.all([observe(reference, test, keep), observe(candidate, test, keep)]);
  const problems = compare(test, a, b);
  if (problems.length > 0) failed++;
  console.log(`${problems.length > 0 ? "✗" : "✓"} ${test.name}`);
  for (const problem of problems) console.log(`    ${problem}`);
}
console.log(`\n${selected.length - failed} of ${selected.length} cases agree`);
process.exit(failed > 0 ? 1 : 0);
