// The JavaScript side of testdata/format: the cases every validator and
// scanner is held to (testdata/format/README.md). The Go tests in
// go/internal/format and go/internal/scan and the Rust tests in
// crates/slink-format read the same two files.
import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { SECRET_PATTERN_SOURCES, validateRun } from "@session-link/format";
import viewerValidate from "../packages/viewer/validate-session.js";

const read = (name) => JSON.parse(readFileSync(new URL(`../testdata/format/${name}`, import.meta.url), "utf8"));
const INVARIANT = /^(duplicate span id|parent_id cycle) /;

for (const item of read("cases.json")) {
  test(`format case: ${item.name}`, () => {
    const expected = typeof item.expect === "string" ? item.expect : item.expect.js;
    let doc;
    try {
      doc = JSON.parse(item.text ?? JSON.stringify(item.doc));
    } catch {
      assert.equal("unreadable", expected);
      return;
    }
    const errors = validateRun(doc);
    assert.equal(errors.length ? "invalid" : "valid", expected, errors.join("\n"));
    if (item.at) assert.ok(errors.some((error) => error.startsWith(`${item.at} `)), `no error at ${item.at}:\n${errors.join("\n")}`);
    if (item.error) assert.deepEqual(errors, [item.error]);
    if (item.count) assert.equal(errors.length, item.count);
    // The viewer ships the schema alone and checks the tree in its own code,
    // so it agrees wherever the tree is not what is wrong.
    assert.equal(viewerValidate(doc), errors.every((error) => INVARIANT.test(error)));
  });
}

// The scan as the hosted service runs it (lib/secrets.ts in the server
// repository): this holds the pattern list to the same findings under a
// JavaScript RegExp.
const patterns = SECRET_PATTERN_SOURCES.map(({ name, source }) => ({ name, re: new RegExp(source, "g") }));
function scanForSecrets(text) {
  const hits = [];
  for (const { name, re } of patterns) {
    for (const match of text.matchAll(re)) {
      hits.push({ pattern: name, preview: `${match[0].slice(0, 8)}…(${match[0].length} chars)` });
      if (hits.length >= 10) return hits;
    }
  }
  return hits;
}

for (const item of read("scan.json")) {
  test(`scan case: ${item.name}`, () => {
    assert.deepEqual(scanForSecrets(item.text), item.hits);
  });
}
