#!/usr/bin/env node
/**
 * Runs the protocol cases (testdata/protocol/README.md) through the viewer's
 * code in a real browser. test/protocol-cases.test.mjs runs the same code on
 * Node, whose Web Crypto is OpenSSL; a browser brings its own library, and
 * the viewer in a browser is what recipients use.
 *
 *   node scripts/check-protocol-browser.mjs
 *
 * BROWSER_BINARY or CHROME_PATH names a Chromium-family browser; without
 * either, scripts/browser.mjs looks for the newest Playwright headless shell,
 * Chrome and Brave. Nothing leaves the machine: the page and the cases are
 * served from loopback.
 */
import { readFileSync } from "node:fs";
import { mkdtemp, rm } from "node:fs/promises";
import http from "node:http";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";
import { findBrowser, launchBrowser } from "./browser.mjs";
import { bundleViewer } from "./viewer-module.mjs";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
if (!findBrowser()) {
  console.error("Set BROWSER_BINARY to a Chromium-family browser executable");
  process.exit(2);
}

const bundled = await bundleViewer(["identity-protocol", "identity-crypto", "encryption", "named-sharing"], { format: "iife", globalName: "viewer", logLevel: "silent" });
const FILES = ["envelopes.json", "statements.json", "histories.json"];
// keys.json is judged whole in the page: what a case opens to is compared
// there, with the secrets it names.
const KEYS = "keys.json";
const served = {
  "/": ["text/html", "<!doctype html><meta charset=utf-8><title>protocol cases</title><script src=/viewer.js></script>"],
  "/viewer.js": ["text/javascript", bundled],
  ...Object.fromEntries([...FILES, KEYS].map((name) => [`/${name}`, ["application/json", readFileSync(path.join(root, "testdata/protocol", name))]])),
};
const server = http.createServer((request, response) => {
  const [type, body] = served[request.url] ?? ["text/plain", ""];
  response.writeHead(served[request.url] ? 200 : 404, { "content-type": type });
  response.end(body);
});
await new Promise((resolve) => server.listen(0, "127.0.0.1", resolve));
const origin = `http://127.0.0.1:${server.address().port}`;

// Runs in the page. It is the judging of test/protocol-cases.test.mjs.
async function judgeAll() {
  const base64 = (bytes) => btoa(Array.from(bytes, (byte) => String.fromCharCode(byte)).join(""));
  const bytes = (text) => Uint8Array.from(atob(text), (c) => c.charCodeAt(0));
  const load = async (name) => (await fetch(`/${name}`)).json();
  const outcome = async (run) => {
    try {
      return { value: await run() };
    } catch (error) {
      // A browser's own error can have no message, and it is still a refusal.
      return { error: String(error?.message || error?.name || error) || "refused" };
    }
  };
  const out = [];
  for (const item of await load("envelopes.json")) {
    const { value, error } = await outcome(() => viewer.decryptDocument(bytes(item.envelope), item.key));
    out.push({ file: "envelopes.json", name: item.name, verdict: error ? "refused" : "opens", error, read: error ? undefined : base64(value) });
  }
  for (const item of await load("statements.json")) {
    const { error } = await outcome(() => viewer.verify(item.kind, { payload: item.payload, signature: item.signature }, item.public));
    out.push({ file: "statements.json", name: item.name, verdict: error ? "refused" : "verified", error });
  }
  const { signed, cases } = await load("histories.json");
  for (const item of cases) {
    const events = item.events.map((id) => signed[id]);
    const { value, error } = await outcome(() =>
      item.vault ? viewer.verifyRecord({ events, vault: signed[item.vault] }, item.account) : viewer.verifyHistory(events, item.account),
    );
    if (error) {
      out.push({ file: "histories.json", name: item.name, verdict: "invalid", error });
      continue;
    }
    const { state, head, vault } = value;
    out.push({
      file: "histories.json",
      name: item.name,
      verdict: "valid",
      read: {
        head,
        seq: state.seq + 0,
        kind: state.kind,
        signer: state.signer,
        epoch: state.epoch,
        inbox: state.inbox || "",
        devices: state.devices.map((device) => ({ id: device.id, name: device.name })),
        ...(vault ? { revision: vault.revision } : {}),
      },
    });
  }

  // keys.json: boxes, wrappers, signatures, device requests, an account
  // opened by a device and by a recovery key, and shares for named people.
  // `same` is whether what was opened is what the case says.
  const keys = await load("keys.json");
  const { unb64, b64, privateKey, unseal, unwrap, keyContext } = viewer;
  const text = (payload) => new TextDecoder().decode(unb64(payload));
  const openVault = (key, state, vault) => unseal(key, vault.data, `slink/vault/v1/${keyContext(state)}`);
  const sameVault = (item, key, contents) => b64(key) === item.vault_key && base64(contents) === item.contents;
  const SECTIONS = {
    boxes: ["opens", (item) => unseal(unb64(item.key), item.box, item.context), (item, plain) => base64(plain) === item.plaintext],
    wrappers: ["opens", async (item) => unwrap(await privateKey("X25519", unb64(item.private)), item.public, item.wrapper, item.context), (item, key) => b64(key) === item.key],
    recipients: ["wraps", (item) => viewer.wrap(item.public, new Uint8Array(32), "context"), (item, wrapper) => wrapper.length === 124],
    signatures: [
      "signed",
      async (item) => {
        const made = await viewer.sign(item.kind, JSON.parse(text(item.payload)), await privateKey("Ed25519", unb64(item.seed)));
        await viewer.verify(item.kind, made, item.public);
        return made;
      },
      (item, made) => made.payload === item.payload && made.signature === item.signature,
    ],
    requests: [
      "verified",
      async (item) => {
        const request = viewer.unpack(item);
        await viewer.verify("request", item, request.sign);
        return request;
      },
      async (item, request) => !item.id || ((await viewer.deviceID(request.sign, request.box)) === item.id && (await viewer.confirmation(item)) === item.code),
    ],
    devices: [
      "opens",
      async (item) => {
        const { state, vault } = await viewer.verifyRecord({ events: item.events, vault: item.vault }, item.account);
        const device = state.devices.find((entry) => entry.id === item.device);
        if (!device) throw new Error("the history does not hold the device");
        const key = await unwrap(await privateKey("X25519", unb64(item.private)), device.box, device.wrap, keyContext(state));
        return { key, contents: await openVault(key, state, vault) };
      },
      (item, { key, contents }) => sameVault(item, key, contents),
    ],
    recoveries: [
      "opens",
      // The recovery branch of browser-identity.ts, as in
      // test/protocol-cases.test.mjs.
      async (item) => {
        const { state, vault } = await viewer.verifyRecord({ events: item.events, vault: item.vault }, item.account);
        const parts = item.token.trim().split(".");
        if (parts.length !== 3 || parts[0] !== "slr1" || parts[1] !== (await viewer.hash(viewer.utf8(`${state.account}\0${state.root}`)))) throw new Error("Recovery key does not match this account");
        const raw = await unseal(unb64(parts[2], 32), state.recovery_box, `slink/recovery/v1/${state.account}/${state.root}`);
        if (raw.length !== 64) throw new Error("Invalid recovery package");
        const root = await privateKey("Ed25519", raw.slice(0, 32));
        await viewer.verify("recovery-check", await viewer.sign("recovery-check", { account: state.account }, root), state.root);
        const key = await unwrap(await privateKey("X25519", raw.slice(32)), state.recovery, state.recovery_wrap, keyContext(state));
        return { raw, key, contents: await openVault(key, state, vault) };
      },
      (item, { raw, key, contents }) => b64(raw.slice(0, 32)) === item.root && b64(raw.slice(32)) === item.agreement && sameVault(item, key, contents),
    ],
    // The viewer is the recipient's side of a share and has no sender's:
    // only the shares that are granted are its to open, as their recipient
    // and as their sender.
    shares: [
      "granted",
      async (item) => {
        const { owner, recipient } = item;
        const share = { id: item.share.id, sha256: item.share.sha256, statement: item.invite, owner_history: owner.history.slice(0, owner.invited), invitation: { id: item.invitation.id, email: item.invitation.email, status: "accepted" }, claim: item.claim, grant: item.grant, grant_history: owner.history };
        const identity = async (who, history) => ({ account: who.account, state: (await viewer.verifyHistory(history, who.account)).state, record: { events: history }, data: { inboxes: who.inboxes } });
        return {
          proof: await viewer.bindingProof(item.invitation.secret, item.claim),
          sender: await viewer.namedContentKey({ ...share, owner: true }, await identity(owner, share.owner_history)),
          recipient: await viewer.namedContentKey({ ...share, owner: false }, await identity(recipient, recipient.history)),
        };
      },
      (item, opened) => opened.proof === item.claim.proof && opened.sender === item.key && opened.recipient === item.key,
      (item) => item.expect === "granted",
    ],
  };
  for (const [section, [accepted, judge, check, mine = () => true]] of Object.entries(SECTIONS)) {
    for (const item of keys[section].filter(mine)) {
      const { value, error } = await outcome(() => judge(item));
      const compared = error ? {} : await outcome(() => check(item, value));
      out.push({ file: "keys.json", section, name: item.name, verdict: error ? "refused" : accepted, error: error ?? compared.error, same: error ? undefined : compared.value === true });
    }
  }
  return { agent: navigator.userAgent, out };
}

const scratch = await mkdtemp(path.join(os.tmpdir(), "slink-protocol-browser-"));
let browser;
let failures = 0;
try {
  browser = await launchBrowser({ profile: scratch });
  const { evaluate, navigate, waitFor } = await browser.firstPage();
  await navigate(`${origin}/`);
  await waitFor("typeof viewer === 'object' && !!crypto.subtle");
  const { agent, out } = await evaluate(`(${judgeAll})()`);

  const expected = Object.fromEntries(
    FILES.map((name) => {
      const file = JSON.parse(served[`/${name}`][1]);
      return [name, new Map((file.cases ?? file).map((item) => [item.name, item]))];
    }),
  );
  const keys = JSON.parse(served[`/${KEYS}`][1]);
  for (const got of out) {
    const item = got.section ? keys[got.section].find((entry) => entry.name === got.name) : expected[got.file].get(got.name);
    // A signature has no verdict: a seed and a payload always have one.
    const want = item.expect === undefined ? "signed" : typeof item.expect === "string" ? item.expect : item.expect.js;
    const read = item.plaintext ?? item.state?.js ?? item.state;
    const same = got.verdict !== want ? false : got.section ? got.same !== false : got.read === undefined || JSON.stringify(got.read) === JSON.stringify(read);
    if (same) continue;
    failures++;
    console.error(`${got.file}: ${got.section ? `${got.section}: ` : ""}${got.name}\n  want ${want}, got ${got.verdict}${got.error ? ` (${got.error})` : ""}${got.verdict === want ? (got.section ? ", and it does not open to what the case says" : `, read ${JSON.stringify(got.read)}`) : ""}`);
  }
  console.log(`${agent}\n${out.length} cases, ${failures} judged differently from what the cases say for the viewer`);
} finally {
  browser?.close();
  server.close();
  await rm(scratch, { recursive: true, force: true }).catch(() => {});
}
process.exit(failures ? 1 : 0);
