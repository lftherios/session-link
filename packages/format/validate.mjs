#!/usr/bin/env node
// Validate session/v0 documents against the schema plus the invariants JSON
// Schema can't express (unique span ids, acyclic parent chains). The single
// source of truth for the format's rules — the server and the CLI both call
// validateRun(). With no arguments (run directly) it checks the bundled
// examples, so "the examples validate" stays executable.
// usage: node validate.mjs [session.json ...]
import { readFile } from "node:fs/promises";
import path from "node:path";
import { fileURLToPath } from "node:url";
import Ajv2020Module from "ajv/dist/2020.js";
import addFormatsModule from "ajv-formats";
// Static JSON import (not a disk read) so the schema inlines when the CLI is
// bundled for distribution — a bundled/binary build has no schema file to read.
import schema from "./session.schema.json" with { type: "json" };

const Ajv2020 = Ajv2020Module.default ?? Ajv2020Module;
const addFormats = addFormatsModule.default ?? addFormatsModule;

const ajv = new Ajv2020({ allErrors: true, allowUnionTypes: true });
addFormats(ajv);
const validateSchema = ajv.compile(schema);

export function invariantErrors(run) {
  const byId = new Map();
  for (const s of run.spans) {
    if (byId.has(s.id)) return [`duplicate span id "${s.id}"`];
    byId.set(s.id, s);
  }
  const acyclic = new Set();
  for (const s of run.spans) {
    const seen = new Set();
    let cur = s;
    while (cur && !acyclic.has(cur.id)) {
      if (seen.has(cur.id)) return [`parent_id cycle involving span "${cur.id}"`];
      seen.add(cur.id);
      cur = cur.parent_id ? byId.get(cur.parent_id) : undefined;
    }
    for (const id of seen) acyclic.add(id);
  }
  return [];
}

/** (data) => string[] of errors; empty means valid. Schema + invariants. */
export function validateRun(data) {
  if (!validateSchema(data)) {
    return (validateSchema.errors ?? [])
      .slice(0, 20)
      .map((e) => `${e.instancePath || "/"} ${e.message}`);
  }
  return invariantErrors(data);
}

// Basename, not import.meta.url equality: when the CLI is bundled into one
// file every module shares import.meta.url, so URL-equality would fire this
// block on `slink <cmd>`. The entry basename is validate.mjs only when this
// file is run directly.
const isMain = path.basename(process.argv[1] ?? "") === "validate.mjs";

if (isMain) {
  const here = path.dirname(fileURLToPath(import.meta.url));
  const files = process.argv.slice(2).length
    ? process.argv.slice(2)
    : ["chat.json", "agent-eval.json"].map((f) => path.join(here, "examples", f));

  let failed = false;
  for (const file of files) {
    const data = JSON.parse(await readFile(file, "utf8"));
    const errors = validateRun(data);
    if (errors.length) {
      failed = true;
      console.error(`✗ ${path.relative(process.cwd(), file)}`);
      for (const e of errors) console.error(`    ${e}`);
    } else {
      console.log(`✓ ${path.relative(process.cwd(), file)}`);
    }
  }
  process.exit(failed ? 1 : 0);
}
