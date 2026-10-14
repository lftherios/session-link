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
  stdin: { contents: 'export * from "./identity-protocol"; export * from "./identity-crypto"; export * from "./encryption"; export * from "./named-sharing";', resolveDir: viewer, loader: "ts" },
  bundle: true,
  write: false,
  format: "esm",
});
const { decryptDocument, verify, verifyHistory, verifyRecord, unpack, unwrap, unseal, privateKey, wrap, sign, confirmation, deviceID, keyContext, hash, utf8, unb64, bindingProof, namedContentKey } =
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

/*
 * testdata/protocol/keys.json: boxes, wrappers, signatures, device requests,
 * and accounts opened by a device and by a recovery key. Go reads the same
 * file in go/internal/identity/keys_test.go and Rust in
 * crates/slink-protocol/tests/keys.rs.
 *
 * The viewer makes its nonces and one-time keys itself, so it cannot make a
 * case again as Rust does. It opens each one, and where a statement is what a
 * client writes it signs the same bytes.
 */
const keys = read("keys.json");
const bytes = (text) => new Uint8Array(Buffer.from(text, "base64url"));
const std = (value) => Buffer.from(value).toString("base64");

/** Runs a section: each case opens to what it says, or is refused. */
async function settle(section, opens, judge, check = () => {}) {
  const cases = keys[section];
  let opened = 0;
  const failures = [];
  for (const item of cases) {
    const want = expected(item);
    const { value, error } = await outcome(() => judge(item));
    if (want === "refused") {
      if (!error) failures.push(`${item.name}: want refused, got ${opens}`);
      continue;
    }
    opened++;
    if (want !== opens) failures.push(`${item.name}: unknown expectation ${want}`);
    else if (error) failures.push(`${item.name}: ${error.message}`);
    else {
      const problem = await outcome(() => check(item, value));
      if (problem.error) failures.push(`${item.name}: ${problem.error.message}`);
    }
  }
  assert.deepEqual(failures, []);
  assert.ok(opened > 0 && opened < cases.length, `${section}: ${opened} of ${cases.length} are accepted; both kinds are needed`);
}

test("boxes open or are refused as the cases say", async () => {
  await settle(
    "boxes",
    "opens",
    (item) => unseal(bytes(item.key), item.box, item.context),
    (item, plain) => assert.equal(std(plain), item.plaintext),
  );
});

test("wrappers open or are refused as the cases say", async () => {
  await settle(
    "wrappers",
    "opens",
    async (item) => unwrap(await privateKey("X25519", bytes(item.private)), item.public, item.wrapper, item.context),
    (item, key) => assert.equal(Buffer.from(key).toString("base64url"), item.key),
  );
  await settle(
    "recipients",
    "wraps",
    (item) => wrap(item.public, new Uint8Array(32), "context"),
    (item, wrapper) => assert.equal(wrapper.length, 124),
  );
});

test("a seed and a payload have one signature", async () => {
  const kinds = new Set();
  for (const item of keys.signatures) {
    const text = Buffer.from(item.payload, "base64url").toString();
    // The payload is what JSON.stringify writes for the value it holds.
    const made = await sign(item.kind, JSON.parse(text), await privateKey("Ed25519", bytes(item.seed)));
    assert.deepEqual(made, { payload: item.payload, signature: item.signature }, item.name);
    await verify(item.kind, made, item.public);
    kinds.add(item.kind);
  }
  assert.equal(kinds.size, 7);
});

test("device requests are verified or refused as the cases say", async () => {
  let made = 0;
  await settle(
    "requests",
    "verified",
    // What the viewer does with a pending request before it looks at the
    // account, the root key and the time.
    async (item) => {
      const request = unpack(item);
      await verify("request", item, request.sign);
      return request;
    },
    async (item, request) => {
      // A case only some implementations verify says nothing more of it.
      if (!item.id) return;
      assert.equal(await deviceID(request.sign, request.box), item.id);
      assert.equal(await confirmation(item), item.code);
      const text = Buffer.from(item.payload, "base64url").toString();
      if (!item.seed || JSON.stringify(request) !== text) return;
      made++;
      assert.deepEqual(await sign("request", request, await privateKey("Ed25519", bytes(item.seed))), { payload: item.payload, signature: item.signature });
    },
  );
  assert.ok(made >= 4, `${made} requests were signed again`);
});

const openVault = (key, state, vault) => unseal(key, vault.data, `slink/vault/v1/${keyContext(state)}`);
const sameVault = (item, key, contents) => {
  assert.equal(Buffer.from(key).toString("base64url"), item.vault_key);
  assert.equal(std(contents), item.contents);
};

test("a device opens its account or is refused", async () => {
  await settle(
    "devices",
    "opens",
    async (item) => {
      const { state, vault } = await verifyRecord({ events: item.events, vault: item.vault }, item.account);
      const device = state.devices.find((entry) => entry.id === item.device);
      if (!device) throw new Error("the history does not hold the device");
      const key = await unwrap(await privateKey("X25519", bytes(item.private)), device.box, device.wrap, keyContext(state));
      return { key, contents: await openVault(key, state, vault) };
    },
    (item, { key, contents }) => sameVault(item, key, contents),
  );
});

test("a recovery key opens its account or is refused", async () => {
  await settle(
    "recoveries",
    "opens",
    // The recovery branch of browser-identity.ts, from the key a person typed
    // to the vault. The rule lives inside the code that talks to the
    // service, so it is repeated here with the viewer's own primitives.
    async (item) => {
      const { state, vault } = await verifyRecord({ events: item.events, vault: item.vault }, item.account);
      const parts = item.token.trim().split(".");
      if (parts.length !== 3 || parts[0] !== "slr1" || parts[1] !== (await hash(utf8(`${state.account}\0${state.root}`)))) throw new Error("Recovery key does not match this account");
      const raw = await unseal(unb64(parts[2], 32), state.recovery_box, `slink/recovery/v1/${state.account}/${state.root}`);
      if (raw.length !== 64) throw new Error("Invalid recovery package");
      const root = await privateKey("Ed25519", raw.slice(0, 32));
      const recovery = await privateKey("X25519", raw.slice(32));
      await verify("recovery-check", await sign("recovery-check", { account: state.account }, root), state.root);
      const key = await unwrap(recovery, state.recovery, state.recovery_wrap, keyContext(state));
      return { raw, key, contents: await openVault(key, state, vault) };
    },
    (item, { raw, key, contents }) => {
      assert.equal(Buffer.from(raw.slice(0, 32)).toString("base64url"), item.root);
      assert.equal(Buffer.from(raw.slice(32)).toString("base64url"), item.agreement);
      sameVault(item, key, contents);
    },
  );
});

/**
 * A share as the service hands it to one of its two sides, with that side's
 * unlocked identity. A case that is refused may hold a recipient's history
 * that is not one, so each side is made only when it is asked for.
 */
function sides(item) {
  const { owner, recipient } = item;
  const share = {
    id: item.share.id,
    sha256: item.share.sha256,
    statement: item.invite,
    // The history as it stood when the invitation was signed.
    owner_history: owner.history.slice(0, owner.invited),
    invitation: { id: item.invitation.id, email: item.invitation.email, status: "accepted" },
    claim: item.claim,
    grant: item.grant,
    grant_history: owner.history,
  };
  const identity = async (who, history) => ({ account: who.account, state: (await verifyHistory(history, who.account)).state, record: { events: history }, data: { inboxes: who.inboxes } });
  return {
    asOwner: async () => [{ ...share, owner: true }, await identity(owner, share.owner_history)],
    asRecipient: async () => [{ ...share, owner: false }, await identity(recipient, recipient.history)],
  };
}

test("a share that is granted opens for its recipient and for its sender", async () => {
  // The viewer is the recipient's side of a named share and has no sender's
  // side: whether a claim is granted is Go's to judge, and Rust's. What it
  // has is the other end: every statement a grant rests on, verified before
  // the key is unwrapped.
  let granted = 0;
  for (const item of keys.shares) {
    assert.equal(await hash(unb64(item.invitation.secret, 32)), item.invitation.commitment, item.name);
    const { asOwner, asRecipient } = sides(item);
    assert.equal(await namedContentKey(...(await asOwner())), item.key, `${item.name}: for the sender`);
    if (item.expect !== "granted") continue;
    granted++;
    assert.equal(await bindingProof(item.invitation.secret, item.claim), item.claim.proof, item.name);
    const [share, identity] = await asRecipient();
    assert.equal(await namedContentKey(share, identity), item.key, `${item.name}: for the recipient`);
    // Without the grant there is nothing to open, and another share's grant
    // is not this one's.
    await assert.rejects(namedContentKey({ ...share, grant: undefined, grant_history: undefined }, identity), item.name);
    await assert.rejects(namedContentKey({ ...share, id: "sharefxtvr2346" }, identity), item.name);
    await assert.rejects(namedContentKey({ ...share, grant_history: share.owner_history.slice(0, 1) }, identity), item.name);
  }
  assert.ok(granted >= 5, `${granted} shares were opened`);
});
