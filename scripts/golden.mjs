#!/usr/bin/env node
/**
 * Sync the Go-embedded artifacts that derive from @session-link/format, so
 * the Go build can't drift from the format package:
 *
 *   packages/format/secret-patterns.json   ← SECRET_PATTERN_SOURCES
 *   go/internal/scan/secret-patterns.json  ← same (go:embed'd by the scanner)
 *   go/internal/format/session.schema.json ← copy of the canonical schema
 *
 *   node scripts/golden.mjs --update   regenerate
 *   node scripts/golden.mjs --check    verify (exit 1 on any drift) — CI runs this
 *
 * History: this began as the JS→Go parity oracle that also GENERATED the
 * testdata fixtures from the (now-removed) JS reference CLI. Those fixtures
 * are frozen and owned by the Go golden tests (the golden_test.go files
 * under go/internal) as a regression contract; only these format-derived
 * embeds are still generated here.
 */
import { copyFile, readFile, writeFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { SECRET_PATTERN_SOURCES } from "../packages/format/secret-patterns.mjs";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const stringify = (o) => JSON.stringify(o, null, 2) + "\n";

const mode = process.argv[2];
if (mode !== "--update" && mode !== "--check") {
  console.error("usage: node scripts/golden.mjs --update | --check");
  process.exit(1);
}

let failures = 0;
async function emitContent(file, content) {
  if (mode === "--update") {
    await writeFile(file, content);
    console.log(`✓ ${path.relative(root, file)}`);
    return;
  }
  const current = await readFile(file, "utf8").catch(() => null);
  if (current !== content) {
    failures++;
    console.error(`✗ ${path.relative(root, file)} — ${current == null ? "missing (run --update)" : "drift"}`);
  }
}

const patternsJson = stringify(SECRET_PATTERN_SOURCES);
await emitContent(path.join(root, "packages", "format", "secret-patterns.json"), patternsJson);
await emitContent(path.join(root, "go", "internal", "scan", "secret-patterns.json"), patternsJson);

// The schema copy: --update copies it, --check diffs it.
const schemaSrc = path.join(root, "packages", "format", "session.schema.json");
const schemaDst = path.join(root, "go", "internal", "format", "session.schema.json");
if (mode === "--update") {
  await copyFile(schemaSrc, schemaDst);
  console.log(`✓ ${path.relative(root, schemaDst)}`);
} else {
  const [a, b] = await Promise.all([
    readFile(schemaSrc, "utf8"),
    readFile(schemaDst, "utf8").catch(() => null),
  ]);
  if (a !== b) {
    failures++;
    console.error(`✗ ${path.relative(root, schemaDst)} — ${b == null ? "missing (run --update)" : "drift from packages/format"}`);
  }
}

if (mode === "--check" && failures) {
  console.error(`\n${failures} artifact(s) out of date — run: node scripts/golden.mjs --update`);
  process.exit(1);
}
if (mode === "--check") console.log("go embeds in sync");
