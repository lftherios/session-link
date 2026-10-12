#!/usr/bin/env node
/**
 * Differential check of the session/v0 validators and credential scanners
 * (docs/rust-migration.md, P1). It derives documents from the fictional
 * fixtures, hands the same text to every implementation, and compares what
 * each makes of it:
 *
 *   js      validateRun in packages/format, what the hosted service runs
 *   viewer  the standalone validator the viewer ships (schema only)
 *   go      the Go CLI's publish gate, through go/internal/verdicts
 *   rust    crates/slink-format, through its `verdicts` example
 *
 *   node scripts/format-diff.mjs            run everything
 *   node scripts/format-diff.mjs --list     every difference, not a summary
 *   node scripts/format-diff.mjs <text>     only the cases whose name contains <text>
 *
 * VERDICTS_GO and VERDICTS_RUST name prebuilt helpers; without them the
 * script runs `go run` and `cargo run`.
 *
 * A verdict is one of `valid`, `invalid` (the validator lists errors) and
 * `unreadable` (the text is not JSON to that implementation). Verdicts must
 * agree, an invariant error must be the same text everywhere, Go and Rust
 * must report schema errors at the same places with every one of them among
 * the places ajv reports, and the scan must report the same findings. The
 * wording of a schema error is each library's own and is not compared.
 *
 * A difference in verdict that is known and decided is listed under
 * `explained` below, and testdata/format/cases.json has a case that pins it.
 * Anything else fails the run.
 *
 * The cases in testdata/format are the contract and run in each language's
 * own tests. This sweep is how a difference nobody thought to write a case
 * for is found; when it finds one, the case goes there.
 */
import { spawnSync } from "node:child_process";
import { closeSync, mkdtempSync, openSync, readFileSync, readdirSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { SECRET_PATTERN_SOURCES, validateRun } from "../packages/format/index.mjs";
import viewerValidate from "../packages/viewer/validate-session.js";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const list = args.includes("--list");
const filter = args.find((arg) => !arg.startsWith("--"));

/* ------------------------------------------------------------- documents */

const cases = [];
const add = (name, text) => cases.push({ name, text });
const addDoc = (name, doc) => add(name, JSON.stringify(doc));

// A number as it is written matters (1.0, 1e2, -0), and JSON.stringify would
// rewrite it, so a mutation plants a marker and the text gets the raw token.
const RAW = "\u0000raw:";
const raw = (token) => RAW + token;
const withRaw = (doc) =>
  JSON.stringify(doc).replace(/"\\u0000raw:((?:[^"\\]|\\.)*)"/g, (_, token) => JSON.parse(`"${token}"`));

const minimal = () => ({
  schema: "session/v0",
  created_at: "2026-07-19T10:00:00Z",
  spans: [{ id: "root", type: "agent" }],
});

function fixtures() {
  const files = [
    "packages/format/examples/chat.json",
    "packages/format/examples/agent-eval.json",
    "testdata/viewer/arrival/session.json",
    "testdata/share/research/session.json",
    "testdata/share/research/excerpt.json",
  ];
  const importers = path.join(root, "testdata", "import");
  for (const harness of readdirSync(importers).sort()) {
    for (const fixture of readdirSync(path.join(importers, harness)).sort()) {
      files.push(`testdata/import/${harness}/${fixture}/golden.run.json`);
    }
  }
  return files.map((file) => ({ file, text: readFileSync(path.join(root, file), "utf8") }));
}

// Free-form by the schema's own description: nothing under these is checked,
// so mutations stop one level in.
const OPAQUE = new Set(["raw", "metadata", "extensions", "params", "arguments", "result", "input_schema", "data"]);

const REPLACEMENTS = [null, true, 0, -1, 1.5, "", "x", [], {}];
const TYPE_NAMES = ["llm_call", "tool_call", "retrieval", "text", "thinking", "image", "tool_result", "data", "blob"];
const NUMBER_TOKENS = ["1.0", "1e2", "-0", "-0.0", "0.5", "1E400", "-1E400", "123456789012345678901234567890"];
const HASH = "sha256:" + "a".repeat(64);

/** Every way one node of a document is changed. `set` replaces the node. */
function* mutationsOf(value, key) {
  yield ["removed", undefined];
  for (const replacement of REPLACEMENTS) yield [`as ${JSON.stringify(replacement)}`, replacement];
  if (typeof value === "string") {
    yield ["as 300 characters", "a".repeat(300)];
    yield ["as a hash", HASH];
    yield ["as a malformed hash", "sha256:XYZ"];
    if (key === "type") for (const name of TYPE_NAMES) yield [`as type ${name}`, name];
  }
  if (typeof value === "number") {
    for (const token of NUMBER_TOKENS) yield [`as the number ${token}`, raw(token)];
  }
  if (Array.isArray(value) && value.length) {
    yield ["with its first item twice", [value[0], ...value]];
    yield ["as 33 items", Array.from({ length: 33 }, () => value[0])];
  }
  if (value && typeof value === "object" && !Array.isArray(value)) {
    yield ["with an unknown field", { ...value, zz_unknown: { nested: [1, "two", null] } }];
  }
}

/**
 * Walks a document and yields each node once per shape, where a shape is the
 * path with array positions replaced by the item's `type`. The same shape in
 * a later fixture is skipped: its mutations would test the same rules.
 */
const seenShapes = new Set();
function* nodes(value, pointer = [], shape = "", opaqueDepth = 0) {
  if (pointer.length && !seenShapes.has(shape)) {
    seenShapes.add(shape);
    yield { pointer, value };
  }
  if (opaqueDepth > 1 || !value || typeof value !== "object") return;
  const entries = Array.isArray(value) ? value.map((item, index) => [index, item]) : Object.entries(value);
  for (const [key, child] of entries) {
    const step = Array.isArray(value) ? `[${typeof child?.type === "string" ? child.type : "*"}]` : `/${key}`;
    const deeper = opaqueDepth || OPAQUE.has(key) ? opaqueDepth + 1 : 0;
    yield* nodes(child, [...pointer, key], shape + step, deeper);
  }
}

function mutated(doc, pointer, replacement) {
  const copy = structuredClone(doc);
  let parent = copy;
  for (const key of pointer.slice(0, -1)) parent = parent[key];
  const last = pointer.at(-1);
  if (replacement !== undefined) parent[last] = replacement;
  else if (Array.isArray(parent)) parent.splice(last, 1);
  else delete parent[last];
  return copy;
}

for (const { file, text } of fixtures()) {
  add(`${file} as it is`, text);
  const doc = JSON.parse(text);
  for (const { pointer, value } of nodes(doc)) {
    for (const [label, replacement] of mutationsOf(value, pointer.at(-1))) {
      add(`${file} /${pointer.join("/")} ${label}`, withRaw(mutated(doc, pointer, replacement)));
    }
  }
}

/* ------------------------------------------- rules with a boundary to find */

const set = (doc, pointer, value) => mutated(doc, pointer, value);
const TIMES = [
  "2026-07-19T10:00:00Z", "2026-07-19T10:00:00z", "2026-07-19t10:00:00Z", "2026-07-19 10:00:00Z",
  "2026-07-19T10:00:00", "2026-07-19T10:00Z", "2026-07-19T10:00:00.5Z", "2026-07-19T10:00:00.123456789Z",
  "2026-07-19T10:00:00.Z", "2026-07-19T10:00:00,5Z", "2026-07-19T10:00:00+02:00", "2026-07-19T10:00:00-00:00",
  "2026-07-19T10:00:00+0200", "2026-07-19T10:00:00+02", "2026-07-19T10:00:00+24:00", "2026-07-19T10:00:00+02:60",
  "2026-07-19T10:00:00 +02:00", "2026-07-19T24:00:00Z", "2026-07-19T23:59:60Z", "2026-07-19T10:59:60Z",
  "2026-12-31T23:59:60Z", "2026-07-19T10:60:00Z", "2026-07-19T10:00:61Z", "2026-02-29T10:00:00Z",
  "2028-02-29T10:00:00Z", "2100-02-29T10:00:00Z", "2000-02-29T10:00:00Z", "2026-02-30T10:00:00Z",
  "2026-04-31T10:00:00Z", "2026-13-01T10:00:00Z", "2026-00-10T10:00:00Z", "2026-07-00T10:00:00Z",
  "2026-07-32T10:00:00Z", "0000-01-01T00:00:00Z", "9999-12-31T23:59:59Z", "10000-01-01T00:00:00Z",
  "26-07-19T10:00:00Z", "2026-7-19T10:00:00Z", "2026-07-19T1:00:00Z", "20260719T100000Z",
  "2026-07-19", "10:00:00Z", "2026-07-19T10:00:00Z ", " 2026-07-19T10:00:00Z", "2026-07-19T10:00:00Z\n",
  "2026-07-19T10:00:00ZZ", "2026-07-19T10:00:00+02:00Z", "２０２６-07-19T10:00:00Z", "2026-07-19T10:00:00\u00a0Z",
  "+026-07-19T10:00:00Z", "2026-+7-19T10:00:00Z", "2026-07-19T+1:00:00Z", "2026-07-19T10:00:00.5", "",
  "not-a-date-time", "2026-07-19T10:00:00.000000000000000000001Z", "2026-07-19T10:00:00+00:00", "2026-07-19T10:00:00−02:00",
  "2026-07-19T10:00:00+23:59", "2026-07-19T10:00:00-23:59", "2026-07-19T10:00:00-24:00", "2026-07-19T10:00:00+2:00",
  "2026-07-19T10:00:00+002:00", "2026-07-19T10:00:00.5+02:00", "2026-07-19T10:00:00.5-0200", "2026-07-19T10:00:00.+02:00",
  "2026-07-19T10:00:00.5 Z", "2026-07-19T10:00:00.5z", "2026-07-19T10:00:00.٥Z", "2026-07-19T１0:00:00Z",
  "2026-07-19T23:59:60+00:00", "2026-07-20T01:59:60+02:00", "2026-07-19T23:59:60+02:00", "2026-07-19T15:59:60-08:00",
  "2026-07-19T23:59:60-08:00", "2026-07-19T00:29:60+00:30", "2026-07-19T23:29:60-00:30", "2026-07-19T23:59:60.5Z",
  "2026-07-19T23:59:61Z", "2026-06-31T10:00:00Z", "2026-09-31T10:00:00Z", "2026-11-31T10:00:00Z", "2026-01-31T10:00:00Z",
  "0000-02-29T00:00:00Z", "-026-07-19T10:00:00Z", "2026-07-+9T10:00:00Z", "2026-07-19T10:+1:00Z", "2026-07-19T10:00:+1Z",
  "2026-07-19T10:00:00+-2:00", "2026-07-19T10:00:00++2:00", "2026-07-19T10:00:00+02:+1", "2026-07-19\t10:00:00Z",
  "2026-07-19\n10:00:00Z", "2026-07-19_10:00:00Z", "2026-07-19TT10:00:00Z", "2026-07-1910:00:00Z", "2026/07/19T10:00:00Z",
  "2026-07-19T10.00.00Z", "2026-07-19T10:00:00Zé", "é026-07-19T10:00:00Z", "2026-07-19Té0:00:00Z", "2026-07-19T10:00:00.5é",
  "2026-07-19T10:00:0éZ",
];
for (const time of TIMES) addDoc(`created_at ${JSON.stringify(time)}`, set(minimal(), ["created_at"], time));
for (const time of TIMES.slice(0, 12)) {
  addDoc(`started_at ${JSON.stringify(time)}`, set(minimal(), ["spans", 0, "started_at"], time));
}

const IDS = [
  "a", "A.b_c-9", "a".repeat(64), "a".repeat(65), "", " ", "a b", "a/b", "a#b", "é", "a\n", "\na", "a\u0000",
  "😀", "a:b", "a,b", "a+b", "a~b", "..", "-", "_", "0",
];
for (const id of IDS) addDoc(`span id ${JSON.stringify(id)}`, set(minimal(), ["spans", 0, "id"], id));

const span = (id, parent) => (parent === undefined ? { id, type: "custom" } : { id, type: "custom", parent_id: parent });
const TREES = {
  "a span that is its own parent": [span("a", "a")],
  "two spans in a cycle": [span("a", "b"), span("b", "a")],
  "three spans in a cycle": [span("a", "b"), span("b", "c"), span("c", "a")],
  "a cycle below a root": [span("root"), span("a", "b"), span("b", "a")],
  "a chain into a cycle": [span("x", "a"), span("a", "b"), span("b", "a")],
  "a parent that names no span": [span("a", "missing")],
  "an empty parent": [span("a", "")],
  "a null parent": [span("a", null)],
  "a duplicate id": [span("a"), span("a")],
  "a duplicate id after others": [span("a"), span("b"), span("c"), span("b")],
  "a duplicate id inside a cycle": [span("a", "b"), span("b", "a"), span("a", "b")],
  "a long chain": Array.from({ length: 500 }, (_, i) => span(`s${i}`, i ? `s${i - 1}` : null)),
};
for (const [name, spans] of Object.entries(TREES)) addDoc(name, { ...minimal(), spans });

// Lengths count characters, not bytes or UTF-16 units: é is two bytes and
// one unit, 😀 is four bytes and two units, and each is one character.
for (const [label, unit] of [["ASCII", "a"], ["two-byte", "é"], ["astral", "😀"]]) {
  for (const [field, limit] of [["name", 256], ["id", 128]]) {
    for (const length of [limit, limit + 1]) {
      addDoc(`${field} of ${length} ${label} characters`, set(minimal(), [field], unit.repeat(length)));
    }
  }
  for (const length of [64, 65]) addDoc(`a tag of ${length} ${label} characters`, { ...minimal(), tags: [unit.repeat(length)] });
}
for (const count of [0, 32, 33]) addDoc(`${count} tags`, { ...minimal(), tags: Array.from({ length: count }, (_, i) => `t${i}`) });
addDoc("no spans", { ...minimal(), spans: [] });

const KEYS = [HASH, "sha256:" + "A".repeat(64), "sha256:" + "a".repeat(63), "sha256:" + "a".repeat(65), "x", "", HASH + "\n", "md5:" + "a".repeat(64), " " + HASH];
for (const key of KEYS) {
  addDoc(`attachment key ${JSON.stringify(key)}`, { ...minimal(), attachments: { [key]: { mime: "text/plain" } } });
  addDoc(`raw_ref ${JSON.stringify(key)}`, set(minimal(), ["spans", 0, "raw_ref"], key));
}
addDoc("an attachment without a mime", { ...minimal(), attachments: { [HASH]: {} } });

const COUNTS = ["0", "-0", "1", "1.0", "1e2", "1E2", "1.5", "-1", "-0.5", "0.0", "1e-1", "10e-1", "1E400", "123456789012345678901234567890", "9007199254740993", "1.0000000000000000000000001", "\"1\"", "null", "true"];
for (const token of COUNTS) {
  add(`metrics.input_tokens ${token}`, withRaw({ ...minimal(), metrics: { input_tokens: raw(token) } }));
  add(`metrics.cost_usd ${token}`, withRaw({ ...minimal(), metrics: { cost_usd: raw(token) } }));
  add(`attachment size ${token}`, withRaw({ ...minimal(), attachments: { [HASH]: { mime: "text/plain", size: raw(token) } } }));
  add(`score value ${token}`, withRaw({ ...minimal(), scores: [{ name: "s", value: raw(token) }] }));
}
for (const value of ["session/v0", "run/v0", "session/v1", "Session/v0", "session/v0 ", "", 0, null, ["session/v0"]]) {
  addDoc(`schema ${JSON.stringify(value)}`, { ...minimal(), schema: value });
}

/* ------------------------------------------------ text that is barely JSON */

const text = JSON.stringify(minimal());
const nested = (depth) => `{"schema":"session/v0","created_at":"2026-07-19T10:00:00Z","spans":[{"id":"root","type":"agent"}],"metadata":{"deep":${"[".repeat(depth)}${"]".repeat(depth)}}}`;
const named = (escaped) => text.replace("{", `{"name":"${escaped}",`);
const TEXTS = {
  "empty text": "",
  "only whitespace": " \n",
  "null": "null",
  "an array": "[]",
  "a string": "\"session/v0\"",
  "a number": "1",
  "an unclosed object": "{",
  "an empty object": "{}",
  "a byte order mark first": "\ufeff" + text,
  "a newline after": text + "\n",
  "whitespace around": " \t\r\n" + text + " \t\r\n",
  "a second value after": text + "{}",
  "text after": text + "x",
  "a trailing comma": text.replace("}]", "},]"),
  "a trailing comma in an object": text.replace("}]}", "}],}"),
  "a comment": "// c\n" + text,
  "single quotes": text.replaceAll("\"", "'"),
  "an unquoted key": text.replace("\"schema\"", "schema"),
  "schema given twice, valid last": text.replace("{", "{\"schema\":7,"),
  "schema given twice, invalid last": text.replace("}]}", "}],\"schema\":7}"),
  "spans given twice, invalid last": text.replace("}]}", "}],\"spans\":[]}"),
  "NaN": text.replace("{", "{\"metadata\":{\"n\":NaN},"),
  "Infinity": text.replace("{", "{\"metadata\":{\"n\":Infinity},"),
  "a number with a leading zero": text.replace("{", "{\"metadata\":{\"n\":01},"),
  "a number ending in a point": text.replace("{", "{\"metadata\":{\"n\":1.},"),
  "a number starting with a point": text.replace("{", "{\"metadata\":{\"n\":.5},"),
  "a number with a plus sign": text.replace("{", "{\"metadata\":{\"n\":+1},"),
  "a hexadecimal number": text.replace("{", "{\"metadata\":{\"n\":0x10},"),
  "an unchecked number too large for a double": text.replace("{", "{\"metadata\":{\"n\":1E400},"),
  "an unchecked number too small for a double": text.replace("{", "{\"metadata\":{\"n\":1E-400},"),
  "an unchecked integer of 30 digits": text.replace("{", "{\"metadata\":{\"n\":123456789012345678901234567890},"),
  "a lone high surrogate escape": named("\\ud83d"),
  "a lone low surrogate escape": named("\\ude00"),
  "a reversed surrogate pair": named("\\ude00\\ud83d"),
  "a surrogate pair escape": named("\\ud83d\\ude00"),
  "a high surrogate before a letter": named("\\ud83dx"),
  "an escaped null character": named("\\u0000"),
  "an unknown escape": named("\\x41"),
  "a short unicode escape": named("\\u12"),
  "a raw tab in a string": named("\t"),
  "a raw newline in a string": named("\n"),
  "a raw line separator in a string": named("\u2028"),
  "a raw delete character in a string": named("\u007f"),
  "a raw null character in a string": named("\u0000"),
  "an uppercase unicode escape": named("\\u00E9"),
  "an escaped slash": named("\\/"),
  "a key with a null character": text.replace("{", "{\"\\u0000\":1,"),
  "an empty key": text.replace("{", "{\"\":1,"),
  ...Object.fromEntries([64, 100, 120, 125, 126, 127, 128, 129, 200, 1000, 5000, 9990, 10010].map((depth) => [`metadata nested ${depth} deep`, nested(depth)])),
};
for (const [name, value] of Object.entries(TEXTS)) add(name, value);

/* --------------------------------------------------------- scanner texts */

const AROUND = ["", " ", "\n", "\"", "'", "=", "/", ":", "-", "_", ".", "a", "Z", "9", "é", "日", "😀", "\u00a0", "\u200b", "ſ", "\\n"];
const TOKENS = [
  "sk-" + "a".repeat(15), "sk-" + "a".repeat(16), "sk-" + "aB3_-".repeat(4), "sk-" + "a".repeat(16) + "-", "sk-" + "a".repeat(16) + "_",
  "SK-" + "a".repeat(16), "sk-" + "é".repeat(16), "sk-" + "a".repeat(8) + "é" + "a".repeat(16),
  "sk_live_" + "a".repeat(15), "sk_live_" + "a".repeat(16), "rk_test_" + "A1".repeat(8), "sk_prod_" + "a".repeat(16), "sk_live_" + "a".repeat(16) + "_x",
  "ghp_" + "a".repeat(19), "ghp_" + "a".repeat(20), "gho_" + "a".repeat(20), "ghu_" + "a".repeat(20), "ghs_" + "a".repeat(20), "ghr_" + "a".repeat(20), "ghx_" + "a".repeat(20),
  "github_pat_" + "a".repeat(19), "github_pat_" + "a_".repeat(10),
  "AKIA" + "A".repeat(15), "AKIA" + "A1".repeat(8), "AKIA" + "A".repeat(17), "akia" + "A".repeat(16), "AKIA" + "a".repeat(16),
  "xoxb-" + "a".repeat(9), "xoxb-" + "a".repeat(10), "xoxp-" + "1-a".repeat(4), "xoxz-" + "a".repeat(10), "xoxb-" + "a".repeat(10) + "-",
  "AIza" + "a".repeat(29), "AIza" + "a".repeat(30), "AIza" + "a-_1".repeat(8),
  "-----BEGIN PRIVATE KEY-----", "-----BEGIN RSA PRIVATE KEY-----", "-----BEGIN OPENSSH PRIVATE KEY-----", "-----BEGIN  PRIVATE KEY-----",
  "-----BEGIN rsa PRIVATE KEY-----", "-----BEGIN PUBLIC KEY-----", "----BEGIN PRIVATE KEY----", "-----BEGIN É PRIVATE KEY-----",
];
for (const token of TOKENS) {
  for (const before of AROUND) add(`scan ${JSON.stringify(before + token)}`, `${before}${token} `);
  for (const after of AROUND) add(`scan ${JSON.stringify(token + after)}`, ` ${token}${after}`);
}
add("scan fifty keys", ("sk-" + "a".repeat(16) + " ").repeat(50));
add("scan every pattern, last first", [...TOKENS].reverse().join("\n"));
add("scan nine and then two", ("ghp_" + "a".repeat(20) + " ").repeat(9) + ("sk-" + "b".repeat(16) + " ").repeat(2));
add("scan a key inside a document", JSON.stringify({ ...minimal(), name: "sk-" + "a".repeat(16) }));
add("scan a key split by an escape", JSON.stringify({ ...minimal(), name: "sk-aaaaaaaa\naaaaaaaa" }));

/* ------------------------------------------------------- implementations */

const selected = filter ? cases.filter((item) => item.name.includes(filter)) : cases;
if (!selected.length) {
  console.error(`no case matches ${JSON.stringify(filter)}`);
  process.exit(2);
}

// The scan as the hosted service runs it (lib/secrets.ts in the server).
const patterns = SECRET_PATTERN_SOURCES.map(({ name, source }) => ({ name, re: new RegExp(source, "g") }));
function scanJs(value) {
  const hits = [];
  for (const { name, re } of patterns) {
    for (const match of value.matchAll(re)) {
      hits.push({ pattern: name, preview: `${match[0].slice(0, 8)}…(${match[0].length} chars)` });
      if (hits.length >= 10) return hits;
    }
  }
  return hits;
}

const INVARIANT = /^(duplicate span id|parent_id cycle) /;
function judgeJs(value) {
  let doc;
  try {
    doc = JSON.parse(value);
  } catch (error) {
    return { verdict: "unreadable", errors: [error.message], secrets: scanJs(value) };
  }
  const errors = validateRun(doc);
  return { verdict: errors.length ? "invalid" : "valid", errors, secrets: scanJs(value), viewer: viewerValidate(doc) };
}

const scratch = mkdtempSync(path.join(os.tmpdir(), "format-diff-"));
function helper(name, prebuilt, command, commandArgs, cwd) {
  const input = path.join(scratch, "input.jsonl");
  const output = path.join(scratch, `${name}.jsonl`);
  const [stdin, stdout] = [openSync(input, "r"), openSync(output, "w")];
  const run = prebuilt
    ? spawnSync(prebuilt, [], { stdio: [stdin, stdout, "inherit"] })
    : spawnSync(command, commandArgs, { cwd, stdio: [stdin, stdout, "inherit"] });
  closeSync(stdin);
  closeSync(stdout);
  if (run.error || run.status !== 0) {
    console.error(`the ${name} helper failed${run.error ? `: ${run.error.message}` : ` with status ${run.status}`}`);
    process.exit(2);
  }
  const lines = readFileSync(output, "utf8").trimEnd().split("\n");
  if (lines.length !== selected.length) {
    console.error(`the ${name} helper answered ${lines.length} of ${selected.length} documents`);
    process.exit(2);
  }
  return lines.map((line) => JSON.parse(line));
}

let results;
try {
  writeFileSync(path.join(scratch, "input.jsonl"), selected.map((item) => JSON.stringify(item.text)).join("\n") + "\n");
  const go = helper("go", process.env.VERDICTS_GO, "go", ["run", "./internal/verdicts"], path.join(root, "go"));
  const rust = helper("rust", process.env.VERDICTS_RUST, "cargo", ["run", "--release", "--locked", "--quiet", "-p", "slink-format", "--example", "verdicts"], root);
  results = selected.map((item, index) => ({ ...item, js: judgeJs(item.text), go: go[index], rust: rust[index] }));
} finally {
  rmSync(scratch, { recursive: true, force: true });
}

/* ------------------------------------------------------------ comparison */

// Known differences, each decided and pinned by a case in
// testdata/format/cases.json. `when` sees one result whose verdicts differ
// and says whether this entry accounts for it.
const refusing = (result) => [result.js, result.go, result.rust].filter((side) => side.verdict === "invalid");
const explained = [
  {
    name: "a number too large for a double: JavaScript reads Infinity, Go and Rust refuse the text",
    when: ({ js, go, rust }) =>
      js.verdict !== "unreadable" && /cannot unmarshal number/.test(go.errors[0]) && /^number out of range/.test(rust.errors[0]),
  },
  {
    // Rust takes a time only when JavaScript and Go both do, so it is never
    // the one that accepts.
    name: "a date-time only one library takes: white space for the T or no colon in the offset (ajv), a sign for a digit (Go)",
    when: (result) =>
      result.rust.verdict === "invalid" && refusing(result).every((side) => side.errors.every((error) => /date-time/.test(error))),
  },
  {
    name: "nesting past 127 levels: Rust refuses the text; Go refuses it past 10,000",
    when: ({ js, go, rust }) =>
      js.verdict === "valid" && /^recursion limit exceeded/.test(rust.errors[0]) && (go.verdict === "valid" || /exceeded max depth/.test(go.errors[0])),
  },
];

const same = (a, b) => JSON.stringify(a) === JSON.stringify(b);
const places = (errors) => [...new Set(errors.map((error) => error.split(" ")[0]))].sort();
function differences(result) {
  const { js, go, rust } = result;
  const found = [];
  if (js.verdict !== go.verdict || js.verdict !== rust.verdict) {
    found.push(`verdict: js ${js.verdict}, go ${go.verdict}, rust ${rust.verdict}`);
  } else if (js.verdict === "invalid") {
    const invariant = [js, go, rust].map((side) => side.errors.filter((error) => INVARIANT.test(error)));
    if (!same(invariant[0], invariant[1]) || !same(invariant[0], invariant[2])) {
      found.push(`invariant errors: js ${JSON.stringify(invariant[0])}, go ${JSON.stringify(invariant[1])}, rust ${JSON.stringify(invariant[2])}`);
    }
    // Go and Rust walk the schema the same way and must name the same
    // places. ajv reports more (it also names the branch that failed), so
    // Rust's places must be among its own.
    if (!invariant[0].length) {
      const [inJs, inGo, inRust] = [js, go, rust].map((side) => places(side.errors));
      if (!same(inGo, inRust)) found.push(`places: go ${inGo.join(" ")}, rust ${inRust.join(" ")}`);
      else if (!inRust.every((place) => inJs.includes(place))) found.push(`places: js ${inJs.join(" ")}, rust ${inRust.join(" ")}`);
    }
  }
  if ("viewer" in js) {
    const schemaValid = js.errors.every((error) => INVARIANT.test(error));
    if (js.viewer !== schemaValid) found.push(`viewer: schema ${js.viewer ? "accepted" : "refused"}, js ${schemaValid ? "accepted" : "refused"}`);
  }
  if (!same(js.secrets, go.secrets) || !same(js.secrets, rust.secrets)) {
    found.push(`scan: js ${JSON.stringify(js.secrets)}, go ${JSON.stringify(go.secrets)}, rust ${JSON.stringify(rust.secrets)}`);
  }
  return found;
}

const counts = new Map();
const unexplained = [];
for (const result of results) {
  const found = differences(result);
  if (!found.length) continue;
  const onlyVerdict = found.length === 1 && found[0].startsWith("verdict:");
  const reason = onlyVerdict && explained.find((entry) => entry.when(result));
  if (reason) counts.set(reason.name, (counts.get(reason.name) ?? 0) + 1);
  else unexplained.push({ result, found });
  if (list || !reason) {
    console.log(`${reason ? "explained" : "DIFFERENT"}  ${result.name}`);
    for (const line of found) console.log(`    ${line}`);
    if (!reason) {
      for (const side of ["js", "go", "rust"]) {
        if (result[side].errors.length) console.log(`    ${side}: ${result[side].errors.slice(0, 3).join(" | ")}`);
      }
    }
  }
}

const verdicts = { valid: 0, invalid: 0, unreadable: 0 };
for (const result of results) verdicts[result.js.verdict]++;
console.log(`\n${results.length} texts: js finds ${verdicts.valid} valid, ${verdicts.invalid} invalid, ${verdicts.unreadable} unreadable`);
for (const [name, count] of counts) console.log(`  explained  ${String(count).padStart(5)}  ${name}`);
if (unexplained.length) {
  console.error(`✗ ${unexplained.length} unexplained difference${unexplained.length === 1 ? "" : "s"}`);
  process.exit(1);
}
console.log("✓ no unexplained difference");
