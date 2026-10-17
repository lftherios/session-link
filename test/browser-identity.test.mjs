import assert from "node:assert/strict";
import { test } from "node:test";
import { importViewer } from "../scripts/viewer-module.mjs";

const { browserIdentity, unlockedIdentity, pair, random, b64, concat, utf8, seal, wrap, sign, head, unpack, keyContext, deviceID, verifyRecord } = await importViewer(["browser-identity", "identity-crypto", "identity-protocol"]);
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

// An honest account shared by several browsers, with pending approval requests.
// A write is taken only against the head and revision it expects, as the
// service's store takes it. `before` runs once ahead of the next write, for
// another device's; `lose` takes a write and loses the answer. `drop`, which no
// honest service does, answers a write as taken and keeps the record as it was.
function sharedAccount() {
  const server = { record: null, requests: [], offline: false, lose: false, drop: false, before: null, posts: 0 };
  server.api = async body => {
    if (body) {
      if (server.offline) throw new Error("Connection lost");
      server.posts++;
      if (body.request) server.requests.push(body.request);
      else {
        if (server.before) { const write = server.before; server.before = null; await write(); }
        const vault = server.record ? unpack(server.record.vault) : null;
        if (body.expected_head !== (vault?.head ?? "") || body.expected_revision !== (vault?.revision ?? 0)) throw Object.assign(new Error("Vault changed"), { status: 409 });
        if (!server.drop) server.record = { events: [...(server.record?.events ?? []), ...(body.event ? [body.event] : [])], vault: body.vault };
        if (server.lose) throw new Error("Connection lost");
      }
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
for (const [name, join] of [["approval", "request"], ["the recovery key", "recover"]]) test(`browser setup lost to another device joins by ${name}`, async t => {
  const server = sharedAccount(), devices = environment(t, server.api), other = new Map();
  server.offline = true;
  await assert.rejects(browserIdentity("setup"), /Connection lost/);
  server.offline = false;
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

// Browser A set up the account and approved B; C has asked to join. Each write
// is made by A, except recovery, which C makes with the recovery key.
async function threeBrowsers(t) {
  const server = sharedAccount(), devices = environment(t, server.api), B = new Map(), C = new Map();
  const first = await browserIdentity("setup", { name: "A" });
  await browserIdentity("confirm-recovery");
  const b = await elsewhere(devices, B, () => browserIdentity("request", { name: "B" }));
  await browserIdentity("approve", { id: b.device_id, code: b.confirmation });
  const c = await elsewhere(devices, C, () => browserIdentity("request", { name: "C" }));
  const inputs = { approve: { id: c.device_id, code: c.confirmation }, revoke: { id: b.device_id }, recover: { recovery_key: first.recovery_key, name: "C" } };
  const as = action => (name, input) => action === "recover" ? elsewhere(devices, C, () => browserIdentity(name, input)) : browserIdentity(name, input);
  return { server, devices, B, C, write: action => as(action)(action, inputs[action]), as, pins: action => (action === "recover" ? C : devices).get(account) };
}
const pinsOf = device => ({ seq: device.seq, head: device.head, revision: device.revision });
// Devices on the account once A approves C, C recovers, or A revokes B.
const listed = { approve: 3, recover: 3, revoke: 1 };

// A write the service answers as taken and then serves the record without is
// refused, from then on, as an older history. Otherwise a revoked device would
// still be listed with no error, and this browser's next write would wrap the
// vault key to it again.
for (const action of ["approve", "recover", "revoke"]) test(`browser refuses a record without the ${action} the service took`, async t => {
  const { server, write, as, pins } = await threeBrowsers(t), older = /older or conflicting key history/;
  server.drop = true;
  await assert.rejects(write(action), older);
  assert.equal(pins(action).seq, server.record.events.length, "the pins must be on the write the service took");
  assert.equal(pins(action).revision, unpack(server.record.vault).revision + 1);
  const posts = server.posts;
  for (const next of ["status", action]) await assert.rejects(as(action)(next), older);
  assert.equal(server.posts, posts, "nothing may be written over a record that lost this browser's write");
});

// A write that never arrived, or that lost to another device's, leaves the
// pins where they were and is made again; one whose answer was lost is read
// back. Pins moved before the answer would lock the browser out here.
for (const failure of ["offline", "conflict", "lost"]) for (const action of ["approve", "recover", "revoke"]) test(`browser ${action} ${{ offline: "that never arrived is made again", conflict: "that lost to another device is made again", lost: "whose answer was lost is read back" }[failure]}`, async t => {
  const { server, devices, B, write, as, pins } = await threeBrowsers(t);
  let others = 0;
  if (failure === "conflict") {
    const d = await elsewhere(devices, new Map(), () => browserIdentity("request", { name: "D" }));
    server.before = () => elsewhere(devices, B, () => browserIdentity("approve", { id: d.device_id, code: d.confirmation }));
    others = 1;
  }
  server.offline = failure === "offline"; server.lose = failure === "lost";
  const before = action === "recover" ? null : pinsOf(pins(action));
  await assert.rejects(write(action), failure === "conflict" ? /Vault changed/ : /Connection lost/);
  server.offline = server.lose = false;
  if (before && failure !== "lost") assert.deepEqual(pinsOf(pins(action)), before);
  const status = failure === "lost" ? await as(action)("status") : await write(action);
  assert.equal(status.state, "ready");
  assert.equal(status.devices.length, listed[action] + others);
});

// An account an older client set up has no incoming-share key until a device
// turns sharing on, which is what a named share's recipient page asks for.
async function olderAccount(t) {
  const server = sharedAccount(), devices = environment(t, server.api);
  const root = await pair("Ed25519", true), recovery = await pair("X25519", true), signing = await pair("Ed25519"), box = await pair("X25519"), key = random();
  const state = { account, seq: 0, prev: "", kind: "init", signer: "root", root: root.public, recovery: recovery.public, recovery_box: await seal(random(), new Uint8Array(64), `slink/recovery/v1/${account}/${root.public}`), recovery_wrap: "", epoch: 1, devices: [] };
  state.devices = [{ id: await deviceID(signing.public, box.public), name: "Older", sign: signing.public, box: box.public, wrap: await wrap(box.public, key, keyContext(state)) }];
  state.recovery_wrap = await wrap(recovery.public, key, keyContext(state));
  const event = await sign("event", state, root.private);
  const vault = await sign("vault", { account, revision: 1, head: await head(event), epoch: 1, signer: "root", data: await seal(key, utf8(JSON.stringify({ version: 1, shares: [] })), "slink/vault/v1/" + keyContext(state)) }, root.private);
  server.record = { events: [event], vault };
  devices.set(account, { sign: signing.private, box: box.private, signPublic: signing.public, boxPublic: box.public, root: "", seq: -1, head: "", revision: 0 });
  return server;
}
for (const drop of [false, true]) test(`browser sharing ${drop ? "the service dropped is refused" : "is turned on"}`, async t => {
  const server = await olderAccount(t);
  assert.equal((await browserIdentity("status")).sharing_ready, false);
  server.drop = drop;
  if (!drop) {
    assert.equal((await browserIdentity("sharing")).sharing_ready, true);
    assert.ok((await unlockedIdentity()).state.inbox);
    return;
  }
  await assert.rejects(browserIdentity("sharing"), /older or conflicting key history/);
  await assert.rejects(unlockedIdentity(), /older or conflicting key history/);
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
