import assert from "node:assert/strict";
import { test } from "node:test";
import { build } from "esbuild";

// Bundle the browser protocol with its Web Crypto helpers so the exact code the
// viewer ships runs here against Node's WebCrypto.
const viewer = new URL("../packages/viewer/", import.meta.url).pathname;
const bundled = await build({ stdin: { contents: 'export * from "./identity-protocol"; export * from "./identity-crypto";', resolveDir: viewer, loader: "ts" }, bundle: true, write: false, format: "esm" });
const { verifyHistory, deviceID, sign, pair, random, b64, head } = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);

test("browser identity: a retired incoming-share key can never be reinstated", async () => {
  const root = await pair("Ed25519"), first = await pair("Ed25519");
  const wrapper = () => b64(random(32)) + "." + b64(random(60));
  const device = async (name, sign) => { const box = b64(random(32)); return { id: await deviceID(sign, box), name, sign, box, wrap: wrapper() }; };
  const laptop = await device("Laptop", first.public);
  const state = { account: "usr_fixture", seq: 0, prev: "", kind: "init", signer: "root", root: root.public, recovery: b64(random(32)), recovery_box: b64(random(92)), recovery_wrap: wrapper(), epoch: 1, devices: [laptop] };
  const initial = { state, events: [await sign("event", state, root.private)] };
  // Each step appends one laptop-signed event derived from the current head.
  const step = async (h, change) => { const state = { ...h.state, ...change, seq: h.state.seq + 1, prev: await head(h.events.at(-1)), signer: laptop.id }; return { state, events: [...h.events, await sign("event", state, first.private)] }; };
  const approve = async h => step(h, { kind: "approve", devices: [...h.state.devices, await device("Extra", (await pair("Ed25519")).public)] });
  const rotate = (h, inbox) => step(h, { kind: "revoke", epoch: h.state.epoch + 1, inbox, recovery_wrap: wrapper(), devices: [{ ...h.state.devices[0], wrap: wrapper() }] });
  const a = b64(random(32)), b = b64(random(32));
  let h = await step(initial, { kind: "sharing", inbox: a });
  h = await approve(h); h = await rotate(h, b); h = await approve(h);
  assert.equal((await verifyHistory(h.events, "usr_fixture")).state.inbox, b);
  await assert.rejects(verifyHistory((await rotate(h, a)).events, "usr_fixture"));
  assert.equal((await verifyHistory((await rotate(h, b64(random(32)))).events, "usr_fixture")).state.epoch, 3);
});
