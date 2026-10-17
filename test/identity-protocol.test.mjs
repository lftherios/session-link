import assert from "node:assert/strict";
import { test } from "node:test";
import { importViewer } from "../scripts/viewer-module.mjs";

// Bundle the browser identity and protocol with their Web Crypto helpers so the
// exact code the viewer ships runs here against Node's WebCrypto.
const { nameFor, verifyHistory, deviceID, sign, pair, random, b64, head } = await importViewer(["browser-identity", "identity-protocol", "identity-crypto"]);

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

test("browser identity: a device is never named with half of a surrogate pair", async () => {
  const half = String.fromCharCode(0xd83d), laptop = String.fromCodePoint(0x1f4bb), lost = String.fromCodePoint(0xfffd);
  // A maxlength that cuts an emoji in two, or a paste, leaves the first half behind.
  assert.equal(nameFor(`Mac ${half}`), `Mac ${lost}`);
  assert.equal(nameFor(`${String.fromCharCode(0xdcbb)} Mac`), `${lost} Mac`);
  assert.equal(nameFor(`Mac ${laptop}`), `Mac ${laptop}`);
  assert.equal(nameFor("  "), "This browser");
  assert.equal(new TextEncoder().encode(nameFor(laptop.repeat(30))).length, 80);
  // The verifier refuses the name the old code would have signed, and takes the new one.
  const root = await pair("Ed25519"), first = await pair("Ed25519");
  const wrapper = () => b64(random(32)) + "." + b64(random(60));
  const history = async name => {
    const box = b64(random(32)), device = { id: await deviceID(first.public, box), name, sign: first.public, box, wrap: wrapper() };
    const state = { account: "usr_fixture", seq: 0, prev: "", kind: "init", signer: "root", root: root.public, recovery: b64(random(32)), recovery_box: b64(random(92)), recovery_wrap: wrapper(), epoch: 1, devices: [device] };
    return [await sign("event", state, root.private)];
  };
  await assert.rejects(verifyHistory(await history(`Mac ${half}`), "usr_fixture"));
  assert.equal((await verifyHistory(await history(nameFor(`Mac ${half}`)), "usr_fixture")).state.devices[0].name, `Mac ${lost}`);
});
