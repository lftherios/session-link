import assert from "node:assert/strict";
import { test } from "node:test";
import { readFile } from "node:fs/promises";
import { build } from "esbuild";
import validate from "../packages/viewer/validate-session.js";

const bundled = await build({ entryPoints: [new URL("../packages/viewer/encryption.ts", import.meta.url).pathname], bundle: true, write: false, format: "esm" });
const { encryptDocument, decryptDocument, decodeKey, withAnchor } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);

test("browser encryption: fixture interoperability, fresh keys, tamper and wrong-key rejection", async () => {
  const fixture = JSON.parse(await readFile(new URL("../testdata/encryption/webcrypto.json", import.meta.url)));
  const envelope = new Uint8Array(Buffer.from(fixture.envelope, "base64"));
  assert.equal(new TextDecoder().decode(await decryptDocument(envelope, fixture.key)), fixture.plaintext);
  const second = await encryptDocument(new TextEncoder().encode(fixture.plaintext));
  assert.notEqual(second.key, fixture.key);
  await assert.rejects(decryptDocument(envelope, second.key));
  for (const offset of [0, 7, 8, 12, 24, envelope.length - 1]) {
    const changed = envelope.slice(); changed[offset] ^= 1;
    await assert.rejects(decryptDocument(changed, fixture.key));
  }
  await assert.rejects(decryptDocument(envelope.slice(0, -1), fixture.key));
  assert.throws(() => decodeKey(fixture.key + "="));
  assert.throws(() => decodeKey(fixture.key + "\n"));
  assert.equal(validate(JSON.parse(fixture.plaintext)), true);
  assert.equal(validate({ schema: "session/v0", spans: [] }), false);
});

test("passage navigation and copied links retain the encryption key", () => {
  const link = "https://session.link/s/23456789abcdef#key=private-key&exchange=old";
  const selected = new URL(withAnchor(link, "step.1"));
  assert.equal(new URLSearchParams(selected.hash.slice(1)).get("key"), "private-key");
  assert.equal(new URLSearchParams(selected.hash.slice(1)).get("span"), "step.1");
  assert.equal(withAnchor(selected.href, ""), "https://session.link/s/23456789abcdef#key=private-key");
});
