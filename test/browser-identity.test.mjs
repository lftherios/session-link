import assert from "node:assert/strict";
import { test } from "node:test";
import { build } from "esbuild";

const viewer = new URL("../packages/viewer/", import.meta.url).pathname;
const bundle = await build({ stdin: { contents: 'export * from "./browser-identity"; export * from "./identity-crypto"; export * from "./identity-protocol";', resolveDir: viewer, loader: "ts" }, bundle: true, write: false, format: "esm" });
const { browserIdentity, pair, random, b64, concat, utf8, seal, wrap, sign, head, unpack, keyContext, verifyRecord } = await import(`data:text/javascript;base64,${Buffer.from(bundle.outputFiles[0].text).toString("base64")}`);
const account = "usr_browser_setup";

// Model only the browser storage and locking boundaries; all cryptography,
// protocol verification and setup actions run the shipped browser code.
function environment(t, api) {
  const devices = new Map();
  const replace = (object, name, value) => {
    const descriptor = Object.getOwnPropertyDescriptor(object, name);
    Object.defineProperty(object, name, { configurable: true, value });
    t.after(() => descriptor ? Object.defineProperty(object, name, descriptor) : delete object[name]);
  };
  replace(globalThis, "indexedDB", { open() {
    const request = { result: { close() {}, transaction() {
      const tx = { objectStore: () => ({
        get(key) { const result = {}; queueMicrotask(() => { result.result = structuredClone(devices.get(key)); result.onsuccess(); }); return result; },
        put(value, key) { const copy = structuredClone(value); queueMicrotask(() => { devices.set(key, copy); tx.oncomplete(); }); },
      }) };
      return tx;
    } } };
    queueMicrotask(() => request.onsuccess()); return request;
  } });
  replace(navigator, "locks", { request: (_name, action) => action() });
  t.mock.method(globalThis, "fetch", async (path, options) => {
    assert.equal(path, "/api/identity");
    const body = options.body === undefined ? undefined : JSON.parse(options.body);
    const response = await api(body, devices);
    return Response.json(response);
  });
  return devices;
}
const response = record => ({ account, record, requests: [] });
const recordOf = body => ({ events: [body.event], vault: body.vault });

// The API receives public device keys and replaces the initial identity with
// its own valid, signed history and server-known vault/incoming-share keys.
async function substitute(body) {
  const state = unpack(body.event), root = await pair("Ed25519", true), recovery = await pair("X25519", true), incoming = await pair("X25519", true), key = random();
  state.root = root.public; state.recovery = recovery.public; state.inbox = incoming.public;
  state.recovery_box = await seal(random(), concat(root.seed, recovery.seed), `slink/recovery/v1/${account}/${state.root}`);
  state.recovery_wrap = await wrap(recovery.public, key, keyContext(state));
  state.devices[0].wrap = await wrap(state.devices[0].box, key, keyContext(state));
  const event = await sign("event", state, root.private);
  const data = await seal(key, utf8(JSON.stringify({ version: 2, shares: [], inboxes: { [incoming.public]: b64(incoming.seed) } })), "slink/vault/v1/" + keyContext(state));
  const vault = await sign("vault", { account, revision: 1, head: await head(event), epoch: 1, signer: "root", data }, root.private);
  const record = { events: [event], vault };
  await verifyRecord(record, account);
  return record;
}

for (const lostResponse of [false, true]) test(`browser setup rejects substituted identity ${lostResponse ? "after reload" : "in acknowledgment"}`, async t => {
  let record = null, posts = 0;
  const devices = environment(t, async (body, storage) => {
    if (body) {
      posts++;
      const saved = storage.get(account), own = unpack(body.event);
      assert.equal(saved.root, own.root, "root must be durable before upload");
      assert.equal(saved.head, await head(body.event));
      assert.deepEqual(saved.pendingSetup, body);
      record = await substitute(body);
      if (lostResponse) throw new Error("Connection lost");
    }
    return response(record);
  });
  await assert.rejects(browserIdentity("setup"), lostResponse ? /Connection lost/ : /recovery identity/);
  const pinned = structuredClone(devices.get(account));
  for (const action of ["status", "setup", "confirm-recovery", "sharing"]) await assert.rejects(browserIdentity(action), /recovery identity/);
  assert.deepEqual(devices.get(account), pinned);
  assert.equal(posts, 1, "no keys may be uploaded after accepting the forged response");
});

for (const committed of [false, true]) test(`browser setup retries ${committed ? "a lost acknowledgment" : "an uncommitted request"} without changing keys`, async t => {
  let record = null;
  const requests = [];
  const devices = environment(t, body => {
    if (body) {
      requests.push(body);
      if (committed || requests.length > 1) record = recordOf(body);
      if (requests.length === 1) throw new Error("Connection lost");
    }
    return response(record);
  });
  await assert.rejects(browserIdentity("setup"), /Connection lost/);
  const original = structuredClone(devices.get(account));
  assert.ok(original.root && original.head && original.pendingSetup);
  const status = await browserIdentity("setup");
  assert.equal(status.state, "ready");
  assert.equal(status.recovery_key, original.pendingRecovery);
  assert.equal(requests.length, committed ? 1 : 2);
  if (!committed) assert.deepEqual(requests[1], requests[0]);
  assert.equal(devices.get(account).root, original.root);
  assert.equal(devices.get(account).head, original.head);
  assert.equal(devices.get(account).pendingSetup, undefined);
  await browserIdentity("confirm-recovery");
  assert.equal(devices.get(account).pendingRecovery, undefined);
  record = null;
  await assert.rejects(browserIdentity("setup"), /identity is missing/);
});

test("browser setup checks recovery tokens retained by older clients", async t => {
  let record = null;
  const devices = environment(t, async body => {
    if (body) { record = await substitute(body); throw new Error("Connection lost"); }
    return response(record);
  });
  await assert.rejects(browserIdentity("setup"), /Connection lost/);
  const saved = devices.get(account);
  saved.root = ""; saved.seq = -1; saved.head = ""; saved.revision = 0; delete saved.pendingSetup;
  await assert.rejects(browserIdentity("setup"), /recovery identity/);
});
