#!/usr/bin/env node
/**
 * Golden-fixture harness — the parity oracle for the Go migration (P0 of
 * docs/go-migration.md). Drives the JS reference implementation's PURE
 * contract surfaces over checked-in inputs and pins their outputs:
 *
 *   testdata/proxy/<case>/    the recorder's call-assembly pipeline
 *       input.meta.json       { kind, streamed, started_at, ended_at, httpStatus }
 *       input.request.json    the raw request body
 *       input.response.sse|.json
 *       golden.span.json      → buildLlmSpan over (parsed, assembled) inputs
 *
 *   testdata/import/<harness>/<case>/   the importers' mapping layer
 *       claude-code|pi|codex: input.session.jsonl   (transcript lines)
 *       opencode|hermes:      input.session.json + input.messages.json (db rows)
 *       golden.run.json       → the harness's builder output
 *
 * Also regenerates packages/format/secret-patterns.json from the .mjs
 * source (the Go implementation consumes the JSON; CI fails on drift).
 *
 *   node scripts/golden.mjs --update   regenerate all goldens
 *   node scripts/golden.mjs --check    verify (exit 1 on any drift)
 *
 * Outputs are deterministic by construction: every builder derives solely
 * from its input files. The Go implementation must reproduce these bytes
 * (same JSON value; comparison is on parsed equality, serialization via
 * this file's stringify).
 */
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { validateRun } from "../packages/format/validate.mjs";
import { SECRET_PATTERN_SOURCES } from "../packages/format/secret-patterns.mjs";
import { parseSseText, assembleAnthropicSse, assembleOpenaiChatSse, assembleResponsesSse, buildLlmSpan } from "../cli/normalize.mjs";
import { appendSpool, assembleSpool } from "../cli/store.mjs";
import { transcriptToRun } from "../cli/import-session.mjs";
import { piSessionToRun } from "../cli/import-pi.mjs";
import { codexRolloutToRun } from "../cli/import-codex.mjs";
import { buildOpencodeRun } from "../cli/import-opencode.mjs";
import { buildHermesRun } from "../cli/import-hermes.mjs";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const TESTDATA = path.join(root, "testdata");
const stringify = (o) => JSON.stringify(o, null, 2) + "\n";

const mode = process.argv[2];
if (mode !== "--update" && mode !== "--check") {
  console.error("usage: node scripts/golden.mjs --update | --check");
  process.exit(1);
}
let failures = 0;
const report = (file, ok, why = "") => {
  if (ok) return;
  failures++;
  console.error(`✗ ${path.relative(root, file)}${why ? ` — ${why}` : ""}`);
};

async function emit(file, content) {
  if (mode === "--update") {
    await writeFile(file, content);
    console.log(`✓ ${path.relative(root, file)}`);
    return;
  }
  const current = await readFile(file, "utf8").catch(() => null);
  report(file, current === content, current == null ? "missing (run --update)" : "drift");
}

/* ----------------------------------------------------- secret patterns */
await emit(
  path.join(root, "packages", "format", "secret-patterns.json"),
  stringify(SECRET_PATTERN_SOURCES),
);
// go:embed can't reach outside the go/ module — these copies are emitted
// here and drift-checked so the embedded artifacts can never diverge.
await emit(
  path.join(root, "go", "internal", "scan", "secret-patterns.json"),
  stringify(SECRET_PATTERN_SOURCES),
);
await emit(
  path.join(root, "go", "internal", "format", "session.schema.json"),
  await readFile(path.join(root, "packages", "format", "session.schema.json"), "utf8"),
);

/* --------------------------------------------------------- proxy cases */
const ASSEMBLERS = {
  "anthropic.messages": assembleAnthropicSse,
  "openai.chat": assembleOpenaiChatSse,
  "openai.responses": assembleResponsesSse,
};

async function listCases(dir) {
  try {
    return (await readdir(dir, { withFileTypes: true })).filter((d) => d.isDirectory()).map((d) => d.name);
  } catch {
    return [];
  }
}

for (const name of await listCases(path.join(TESTDATA, "proxy"))) {
  const dir = path.join(TESTDATA, "proxy", name);
  try {
    const meta = JSON.parse(await readFile(path.join(dir, "input.meta.json"), "utf8"));
    const request = JSON.parse(await readFile(path.join(dir, "input.request.json"), "utf8"));
    let response;
    let assembled = true;
    if (meta.streamed) {
      const sse = await readFile(path.join(dir, "input.response.sse"), "utf8");
      response = ASSEMBLERS[meta.kind](parseSseText(sse));
      if (!response) {
        assembled = false;
        response = { unassembled_sse: sse.slice(0, 100_000) };
      }
    } else {
      response = JSON.parse(await readFile(path.join(dir, "input.response.json"), "utf8"));
    }
    // Mirrors cli/proxy.mjs recordCall exactly — this IS the contract.
    const span = buildLlmSpan(meta.kind, {
      id: "s1",
      parent_id: "root",
      request,
      response,
      started_at: meta.started_at,
      ended_at: meta.ended_at,
      httpStatus: meta.httpStatus,
      streamed: meta.streamed,
      captureGap: !assembled,
      transportError: meta.transportError ?? null,
    });
    await emit(path.join(dir, "golden.span.json"), stringify(span));
  } catch (e) {
    report(dir, false, e.message);
  }
}

/* -------------------------------------------------------- import cases */
const IMPORTERS = {
  "claude-code": async (dir) => {
    const lines = (await readFile(path.join(dir, "input.session.jsonl"), "utf8")).split("\n").filter(Boolean);
    return transcriptToRun(lines, "golden");
  },
  pi: async (dir) => {
    const lines = (await readFile(path.join(dir, "input.session.jsonl"), "utf8")).split("\n").filter(Boolean);
    return piSessionToRun(lines, "golden");
  },
  codex: async (dir) => {
    const lines = (await readFile(path.join(dir, "input.session.jsonl"), "utf8")).split("\n").filter(Boolean);
    return codexRolloutToRun(lines, "golden");
  },
  opencode: async (dir) => {
    const session = JSON.parse(await readFile(path.join(dir, "input.session.json"), "utf8"));
    const messages = JSON.parse(await readFile(path.join(dir, "input.messages.json"), "utf8"));
    return buildOpencodeRun(session, messages);
  },
  hermes: async (dir) => {
    const session = JSON.parse(await readFile(path.join(dir, "input.session.json"), "utf8"));
    const messages = JSON.parse(await readFile(path.join(dir, "input.messages.json"), "utf8"));
    return buildHermesRun(session, messages);
  },
};

for (const harness of await listCases(path.join(TESTDATA, "import"))) {
  const build = IMPORTERS[harness];
  if (!build) {
    report(path.join(TESTDATA, "import", harness), false, "unknown harness");
    continue;
  }
  for (const name of await listCases(path.join(TESTDATA, "import", harness))) {
    const dir = path.join(TESTDATA, "import", harness, name);
    try {
      const run = await build(dir);
      const errors = validateRun(run);
      if (errors.length) {
        report(dir, false, `golden does not validate: ${JSON.stringify(errors[0])}`);
        continue;
      }
      await emit(path.join(dir, "golden.run.json"), stringify(run));
    } catch (e) {
      report(dir, false, e.message);
    }
  }
}

/* --------------------------------------------------------- spool cases */
// The spool-layer contract (docs/spool-protocol.md): input.spool is copied
// into a temp capture dir and assembled by the JS reference; the golden is
// the resulting session/v0 document. Parity for Go is PARSED equality —
// assembled bytes are implementation-specific (key order), the document is
// the contract. meta: { mode: "finalize"|"snapshot", endedAt? }.
import { cp, mkdtemp, rm as rmDir } from "node:fs/promises";
import os from "node:os";
for (const name of await listCases(path.join(TESTDATA, "spool"))) {
  const dir = path.join(TESTDATA, "spool", name);
  const tmpDir = await mkdtemp(path.join(os.tmpdir(), "slink-golden-"));
  try {
    const meta = JSON.parse(await readFile(path.join(dir, "input.meta.json"), "utf8"));
    const cap = path.join(tmpDir, "cap.json");
    await cp(path.join(dir, "input.spool"), cap + ".spool");
    const n = await assembleSpool(cap, meta.mode === "finalize" ? { finalize: true, endedAt: meta.endedAt } : {});
    const doc = n == null ? { refused: true } : JSON.parse(await readFile(cap, "utf8"));
    await emit(path.join(dir, "golden.doc.json"), stringify({ spans: n, doc }));
  } catch (e) {
    report(dir, false, e.message);
  } finally {
    await rmDir(tmpDir, { recursive: true, force: true });
  }
}

await mkdir(TESTDATA, { recursive: true });
if (mode === "--check" && failures) {
  console.error(`\n${failures} golden(s) out of date — run: node scripts/golden.mjs --update`);
  process.exit(1);
}
if (mode === "--check") console.log("goldens clean");
