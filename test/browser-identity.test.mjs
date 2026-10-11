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
        delete(key) { queueMicrotask(() => { devices.delete(key); tx.oncomplete(); }); },
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
  for (const action of ["status", "setup", "confirm-recovery", "sharing", "discard-setup"]) await assert.rejects(browserIdentity(action), /recovery identity/);
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
  for (const action of ["setup", "discard-setup"]) await assert.rejects(browserIdentity(action), /recovery identity/);
});

// An honest account shared by several browsers, with pending approval requests.
function sharedAccount() {
  const server = { record: null, requests: [], offline: false, posts: 0 };
  server.api = body => {
    if (body) {
      if (server.offline) throw new Error("Connection lost");
      server.posts++;
      if (body.request) server.requests.push(body.request);
      else server.record = { events: [...(server.record?.events ?? []), ...(body.event ? [body.event] : [])], vault: body.vault };
    }
    return { account, record: server.record, requests: server.requests };
  };
  return server;
}
// Run one action as a second browser that has its own key storage.
async function elsewhere(devices, other, action) {
  const mine = new Map(devices);
  devices.clear(); for (const entry of other) devices.set(...entry);
  try { return await action(); } finally {
    other.clear(); for (const entry of devices) other.set(...entry);
    devices.clear(); for (const entry of mine) devices.set(...entry);
  }
}
const conflict = error => error.code === "setup_conflict";

// A browser whose own setup never reached the server must not stay locked out
// once another device has set the account up. It refuses that identity until
// the unfinished setup is discarded, then joins with fresh keys.
for (const [name, legacy, join] of [["approval", false, "request"], ["the recovery key", false, "recover"], ["the recovery key after an older client", true, "recover"]]) test(`browser setup lost to another device joins by ${name}`, async t => {
  const server = sharedAccount(), devices = environment(t, server.api), other = new Map();
  server.offline = true;
  await assert.rejects(browserIdentity("setup"), /Connection lost/);
  server.offline = false;
  if (legacy) { const saved = devices.get(account); saved.root = ""; saved.seq = -1; saved.head = ""; saved.revision = 0; delete saved.pendingSetup; }
  const unfinished = structuredClone(devices.get(account));
  const first = await elsewhere(devices, other, () => browserIdentity("setup", { name: "Other" }));
  assert.equal(first.state, "ready");
  const posts = server.posts;
  for (const action of ["status", "setup", "request", "recover", "sharing", "confirm-recovery"]) await assert.rejects(browserIdentity(action, { recovery_key: first.recovery_key }), conflict);
  assert.deepEqual(devices.get(account), unfinished);
  assert.equal(server.posts, posts, "an undecided conflict must not upload anything");
  const discarded = await browserIdentity("discard-setup");
  assert.equal(discarded.state, "locked");
  assert.equal(discarded.device_id, "");
  assert.equal(discarded.recovery_pending, false);
  assert.equal(devices.has(account), false);
  if (join === "request") {
    const pending = await browserIdentity("request", { name: "This browser" });
    assert.ok(pending.confirmation);
    await elsewhere(devices, other, () => browserIdentity("approve", { id: pending.device_id, code: pending.confirmation }));
  } else await browserIdentity("recover", { recovery_key: first.recovery_key });
  const joined = await browserIdentity("status");
  assert.equal(joined.state, "ready");
  assert.equal(joined.devices.length, 2);
  assert.notEqual(devices.get(account).signPublic, unfinished.signPublic, "keys the server has seen must not be reused");
  assert.notEqual(devices.get(account).boxPublic, unfinished.boxPublic);
  assert.equal(devices.get(account).pendingRecovery, undefined);
});

// Discarding is only for a setup that was never acknowledged. A browser whose
// identity the server did acknowledge keeps refusing a replacement, and keeps
// the only copy of a recovery key it has not shown yet.
test("browser discards only a setup that was never acknowledged", async t => {
  const server = sharedAccount(), devices = environment(t, server.api), other = new Map();
  await browserIdentity("setup");
  await assert.rejects(browserIdentity("discard-setup"), /no unfinished/);
  const own = server.record;
  server.record = null;
  await elsewhere(devices, other, () => browserIdentity("setup"));
  const replaced = server.record;
  for (const confirmed of [false, true]) {
    const saved = structuredClone(devices.get(account));
    assert.equal(saved.pendingRecovery === undefined, confirmed);
    for (const action of ["discard-setup", "status"]) await assert.rejects(browserIdentity(action), error => !conflict(error) && /recovery identity/.test(error.message));
    assert.deepEqual(devices.get(account), saved);
    if (!confirmed) { server.record = own; await browserIdentity("confirm-recovery"); server.record = replaced; }
  }
});
