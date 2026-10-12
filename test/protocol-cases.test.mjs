import assert from "node:assert/strict";
import { readFileSync } from "node:fs";
import { test } from "node:test";
import { isDeepStrictEqual } from "node:util";
import { build } from "esbuild";

// The cases every implementation of the protocol is held to
// (testdata/protocol/README.md), run here against the code the viewer ships:
// its envelope, its signed statements and its history and vault rules, on
// Node's Web Crypto. Go reads the same files in go/internal/sealed and
// go/internal/identity, and Rust in crates/slink-protocol/tests/cases.rs.
const viewer = new URL("../packages/viewer/", import.meta.url).pathname;
const bundled = await build({
  stdin: { contents: 'export * from "./identity-protocol"; export * from "./identity-crypto"; export * from "./encryption";', resolveDir: viewer, loader: "ts" },
  bundle: true,
  write: false,
  format: "esm",
});
const { decryptDocument, verify, verifyHistory, verifyRecord, unpack, unwrap, unseal, privateKey } =
  await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);

const read = (name) => JSON.parse(readFileSync(new URL(`../testdata/protocol/${name}`, import.meta.url), "utf8"));
// One verdict for every implementation, or one for each.
const expected = (item) => (typeof item.expect === "string" ? item.expect : item.expect.js);
const outcome = async (run) => {
  try {
    return { value: await run() };
  } catch (error) {
    return { error };
  }
};

test("envelopes open or are refused as the cases say", async () => {
  const cases = read("envelopes.json");
  assert.ok(cases.some((item) => item.expect === "opens") && cases.some((item) => item.expect === "refused"));
  for (const item of cases) {
    const { value, error } = await outcome(() => decryptDocument(new Uint8Array(Buffer.from(item.envelope, "base64")), item.key));
    if (expected(item) === "refused") assert.ok(error, `${item.name}: opened`);
    else assert.equal(error ? `refused: ${error.message}` : Buffer.from(value).toString("base64"), item.plaintext, item.name);
  }
});

test("signed statements verify or are refused as the cases say", async () => {
  const cases = read("statements.json");
  assert.ok(cases.some((item) => item.expect === "verified") && cases.some((item) => item.expect === "refused"));
  for (const item of cases) {
    const { error } = await outcome(() => verify(item.kind, { payload: item.payload, signature: item.signature }, item.public));
    assert.equal(error ? "refused" : "verified", expected(item), `${item.name}${error ? `: ${error.message}` : ""}`);
  }
});

/** What the viewer makes of a history, with its vault when the case has one. */
async function judge(signed, item) {
  const events = item.events.map((id) => signed[id]);
  const { value, error } = await outcome(() =>
    item.vault ? verifyRecord({ events, vault: signed[item.vault] }, item.account) : verifyHistory(events, item.account),
  );
  if (error) return { verdict: "invalid", error: error.message };
  const { state, head, vault } = value;
  return {
    verdict: "valid",
    state: {
      head,
      // A payload may say -0, which JavaScript keeps apart from 0 when comparing.
      seq: state.seq + 0,
      kind: state.kind,
      signer: state.signer,
      epoch: state.epoch,
      inbox: state.inbox || "",
      devices: state.devices.map((device) => ({ id: device.id, name: device.name })),
      ...(vault ? { revision: vault.revision } : {}),
    },
  };
}

test("device histories and vault records are judged as the cases say", async () => {
  const { signed, cases } = read("histories.json");
  const count = { valid: 0, invalid: 0, differ: 0 };
  const failures = [];
  for (const item of cases) {
    const want = expected(item);
    count[typeof item.expect === "string" ? want : "differ"]++;
    const got = await judge(signed, item);
    if (got.verdict !== want) failures.push(`${item.name}: want ${want}, got ${got.verdict}${got.error ? ` (${got.error})` : ""}`);
    else if (want === "valid" && !isDeepStrictEqual(got.state, item.state.js ?? item.state)) failures.push(`${item.name}: read ${JSON.stringify(got.state)}`);
  }
  assert.deepEqual(failures, []);
  assert.ok(count.valid > 20 && count.invalid > 100 && count.differ > 20, JSON.stringify(count));
});

test("the wrappers and the vault in the cases open with the keys their labels give", async () => {
  // The generator makes them with node:crypto, not with the viewer's code, so
  // this is the check that it wrote what a client writes.
  const { account } = await import("../scripts/protocol-cases.mjs");
  const { signed, cases } = read("histories.json");
  const item = cases.find((entry) => entry.name === "a vault written by the first device");
  const fixture = account(item.account);
  const laptop = fixture.device("laptop", "Laptop");
  const state = unpack(signed[item.events.at(-1)]);
  const context = `${state.account}/${state.root}/${state.epoch}`;
  const key = await unwrap(await privateKey("X25519", new Uint8Array(laptop.box.seed)), laptop.box.public, state.devices[0].wrap, context);
  assert.deepEqual(Buffer.from(key), fixture.vaultKey(1));
  const recovered = await unwrap(await privateKey("X25519", new Uint8Array(fixture.recovery.seed)), state.recovery, state.recovery_wrap, context);
  assert.deepEqual(Buffer.from(recovered), fixture.vaultKey(1));
  const vault = unpack(signed[item.vault]);
  const contents = await unseal(key, vault.data, `slink/vault/v1/${context}`);
  assert.deepEqual(JSON.parse(Buffer.from(contents).toString()), { version: 1, shares: [] });
});
