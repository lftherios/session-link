#!/usr/bin/env node
/**
 * Record the surface the Rust port has to keep (docs/rust-migration.md, P0),
 * read from the sources so it cannot drift from them:
 *
 *   testdata/compat/cli.json     ← commands, aliases, flags and environment
 *                                  variables of the Go CLI
 *   testdata/compat/hosted.json  ← routes, pages, rewrites and static files of
 *                                  the hosted service
 *
 *   node scripts/compat-inventory.mjs --update [--server ../session-link-server]
 *   node scripts/compat-inventory.mjs --check  [--server ../session-link-server]
 *
 * --check exits 1 on any drift — CI runs it. The hosted service is another
 * repository, so hosted.json is only read or written when --server names a
 * checkout of it. The extraction is by pattern; when the source changes shape
 * it stops with an error instead of recording less.
 */
import { execFileSync } from "node:child_process";
import { mkdir, readFile, readdir, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const out = path.join(root, "testdata", "compat");
const stringify = (o) => JSON.stringify(o, null, 2) + "\n";

const argv = process.argv.slice(2);
const mode = argv.find((a) => a === "--update" || a === "--check");
const serverArg = argv.indexOf("--server");
const server = serverArg >= 0 ? argv[serverArg + 1] : null;
if (!mode || (serverArg >= 0 && !server)) {
  console.error("usage: node scripts/compat-inventory.mjs --update | --check [--server <session-link-server checkout>]");
  process.exit(1);
}

const quoted = (list) => [...list.matchAll(/"([^"]+)"/g)].map((m) => m[1]);
function need(found, what) {
  if (!found) throw new Error(`cannot find ${what}: the source changed shape, so the extraction in this script has to change with it`);
  return found;
}
async function walk(dir) {
  const files = [];
  for (const entry of (await readdir(dir, { withFileTypes: true })).sort((a, b) => (a.name < b.name ? -1 : 1))) {
    const full = path.join(dir, entry.name);
    if (entry.isDirectory()) files.push(...(await walk(full)));
    else files.push(full);
  }
  return files;
}
const goSource = (file) => file.endsWith(".go") && !file.endsWith("_test.go");

async function cli() {
  const dir = path.join(root, "go", "cmd", "slink");
  const read = (name) => readFile(path.join(dir, name), "utf8");
  const [help, main] = await Promise.all([read("help.go"), read("main.go")]);

  const listed = quoted(need(help.match(/var commands = \[\]string\{([\s\S]*?)\n\}/), "the commands list in help.go")[1]);
  const spellings = (word) => quoted(need(main.match(new RegExp(`case ("${word}"[^:\\n]*):`)), `the spellings of ${word} in main.go`)[1]);
  const dispatch = need(main.match(/func dispatch\([\s\S]*?\n\}\n/), "dispatch() in main.go")[0];
  const data = new Set(quoted(need(dispatch.match(/case ([^:\n]+):\n\s*dataCommand = true/), "the data commands in dispatch()")[1]));
  // The first name of a case is the command; the others are its aliases.
  const groups = [...dispatch.matchAll(/case ([^:\n]+):\n\s*run\w+\(args\)/g)].map((m) => quoted(m[1]));

  const dispatched = [...groups.flat(), "version", "help"].sort();
  if (stringify(dispatched) !== stringify([...listed].sort())) {
    throw new Error(`the commands list in help.go and the switch in dispatch() disagree:\n  help.go:  ${[...listed].sort().join(" ")}\n  dispatch: ${dispatched.join(" ")}`);
  }

  const sets = new Map();
  for (const file of (await readdir(dir)).filter(goSource).sort()) {
    let current = null;
    for (const line of (await read(file)).split("\n")) {
      const set = line.match(/flag\.NewFlagSet\("([^"]+)"/);
      if (set) {
        current = { synopsis: "", flags: [] };
        sets.set(set[1], current);
        continue;
      }
      if (!current) continue;
      const flag = line.match(/fs\.(Int|Bool|String)\("([^"]+)",\s*([^,]+),/);
      if (flag) current.flags.push({ name: flag[2], type: flag[1].toLowerCase(), default: JSON.parse(flag[3]) });
      const usage = line.match(/setUsage\(fs, (?:"((?:[^"\\]|\\.)*)"|`([^`]*)`)/);
      if (usage) current.synopsis = usage[2] ?? JSON.parse(`"${usage[1]}"`);
    }
  }
  const commands = groups
    .map(([name, ...aliases]) => {
      const set = need(sets.get(name), `a flag set for ${name}`);
      sets.delete(name);
      return { name, aliases, local_data: data.has(name), synopsis: need(set.synopsis, `the synopsis of ${name}`), flags: set.flags };
    })
    .sort((a, b) => listed.indexOf(a.name) - listed.indexOf(b.name));
  if (sets.size > 0) throw new Error(`flag sets with no command in dispatch(): ${[...sets.keys()].join(" ")}`);

  // Every quoted ALL_CAPS name with an underscore in the Go source is an
  // environment variable today; that is what this pattern relies on.
  const environment = new Map();
  for (const sub of ["cmd", "internal"]) {
    for (const file of (await walk(path.join(root, "go", sub))).filter(goSource)) {
      for (const [, name] of (await readFile(file, "utf8")).matchAll(/"([A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+)"/g)) {
        environment.set(name, (environment.get(name) ?? new Set()).add(path.relative(root, file).split(path.sep).join("/")));
      }
    }
  }
  return {
    commands,
    help: spellings("help"),
    version: spellings("version"),
    environment: [...environment].sort(([a], [b]) => (a < b ? -1 : 1)).map(([name, files]) => ({ name, files: [...files].sort() })),
  };
}

async function hosted(dir) {
  const app = path.join(dir, "app");
  const url = (file) =>
    "/" + path.relative(app, path.dirname(file)).split(path.sep).filter(Boolean).map((part) => part.replace(/^\[(.+)\]$/, ":$1")).join("/");
  const routes = [];
  for (const file of await walk(app)) {
    if (path.basename(file) === "page.tsx") routes.push({ path: url(file), kind: "page", methods: ["GET"] });
    if (path.basename(file) !== "route.ts") continue;
    const methods = [...(await readFile(file, "utf8")).matchAll(/export (?:async function|const) (GET|POST|PUT|PATCH|DELETE|HEAD|OPTIONS)\b/g)].map((m) => m[1]);
    if (methods.length === 0) throw new Error(`no exported method found in ${file}`);
    routes.push({ path: url(file), kind: "route", methods: [...new Set(methods)].sort() });
  }
  routes.sort((a, b) => (a.path < b.path ? -1 : 1));
  const config = await readFile(path.join(dir, "next.config.mjs"), "utf8");
  const rewrites = [...config.matchAll(/\{ source: "([^"]+)", destination: "([^"]+)" \}/g)].map(([, source, destination]) => ({ source, destination }));
  need(rewrites.length > 0, "the rewrites in next.config.mjs");
  const files = (await walk(path.join(dir, "public")))
    .map((file) => "/" + path.relative(path.join(dir, "public"), file).split(path.sep).join("/"))
    .filter((file) => !path.basename(file).startsWith("."))
    .sort();
  return {
    revision: execFileSync("git", ["-C", dir, "rev-parse", "HEAD"], { encoding: "utf8" }).trim(),
    routes,
    rewrites,
    static: files,
  };
}

let failures = 0;
// comparable drops what may differ without the surface having changed.
async function emit(name, content, comparable = (text) => text) {
  const file = path.join(out, name);
  const shown = path.relative(root, file);
  if (mode === "--update") {
    await mkdir(out, { recursive: true });
    await writeFile(file, content);
    console.log(`✓ ${shown}`);
    return;
  }
  const current = await readFile(file, "utf8").catch(() => null);
  if (current == null || comparable(current) !== comparable(content)) {
    failures++;
    console.error(`✗ ${shown} — ${current == null ? "missing (run --update)" : "drift (run --update and read the diff)"}`);
  }
}

await emit("cli.json", stringify(await cli()));
// The server's revision moves with every commit there; only a change to what
// it serves is drift.
if (server) await emit("hosted.json", stringify(await hosted(server)), (text) => text.replace(/^  "revision": "[0-9a-f]+",\n/m, ""));

if (mode === "--check") {
  if (failures > 0) process.exit(1);
  console.log("compat inventory in sync");
}
