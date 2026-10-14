#!/usr/bin/env node
/**
 * Writes the protocol cases in testdata/protocol: the envelopes, signed
 * statements, device histories and vault records that the Go CLI, the viewer
 * and the Rust crate must all judge alike, and the boxes, wrappers, device
 * requests and recovery keys they must all open alike
 * (testdata/protocol/README.md).
 *
 *   node scripts/protocol-cases.mjs --check    the files are what this writes
 *   node scripts/protocol-cases.mjs --update   write them
 *
 * A case here needs a signature, so unlike the format cases these cannot be
 * kept by hand. Every key is derived from a label by SHA-256 and every nonce
 * likewise, so the output is the same on every run and no key here was ever
 * anyone's. The wrappers and vaults are real and open with the keys the
 * labels give.
 *
 * Nothing here calls the implementations under test. The signatures, the
 * envelopes and the wrappers are made with node:crypto directly, which makes
 * this a fourth reading of the wire format and not a copy of one of the
 * three. scripts/protocol-diff.mjs imports the builders for its sweep.
 */
import {
  createCipheriv,
  createHash,
  createHmac,
  createPrivateKey,
  createPublicKey,
  diffieHellman,
  hkdfSync,
  sign as signWith,
  verify as verifyWith,
} from "node:crypto";
import { readFileSync, writeFileSync } from "node:fs";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";

const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const directory = path.join(root, "testdata/protocol");

/* ------------------------------------------------------------ primitives */

export const b64 = (bytes) => Buffer.from(bytes).toString("base64url");
export const unb64 = (text) => Buffer.from(text, "base64url");
export const digest = (bytes) => createHash("sha256").update(bytes).digest("base64url");

/** Bytes that depend on nothing but the label. */
export function material(label, length = 32) {
  const blocks = [];
  for (let i = 0; i * 32 < length; i++) {
    blocks.push(createHash("sha256").update(`session.link fixture/${label}/${i}`).digest());
  }
  return Buffer.concat(blocks).subarray(0, length);
}

const PKCS8 = { ed: "302e020100300506032b657004220420", x: "302e020100300506032b656e04220420" };
const SPKI = { ed: "302a300506032b6570032100", x: "302a300506032b656e032100" };
const publicKey = (type, raw) =>
  createPublicKey({ key: Buffer.concat([Buffer.from(SPKI[type], "hex"), raw]), format: "der", type: "spki" });

/** An Ed25519 (`ed`) or X25519 (`x`) key pair whose seed is the label's bytes. */
export function keyPair(type, label) {
  const seed = material(label);
  const key = createPrivateKey({ key: Buffer.concat([Buffer.from(PKCS8[type], "hex"), seed]), format: "der", type: "pkcs8" });
  const raw = createPublicKey(key).export({ format: "der", type: "spki" }).subarray(12);
  return { seed, private: key, public: b64(raw) };
}

/** AES-256-GCM with the context as associated data: nonce, ciphertext, tag. */
export function seal(key, plain, context, label) {
  const nonce = material(`nonce/${label}`, 12);
  const cipher = createCipheriv("aes-256-gcm", key, nonce);
  cipher.setAAD(Buffer.from(context));
  return b64(Buffer.concat([nonce, cipher.update(plain), cipher.final(), cipher.getAuthTag()]));
}

/** A key wrapped to an X25519 public key, as go/internal/identity wraps one. */
export function wrap(recipient, key, context, label) {
  const ephemeral = keyPair("x", `ephemeral/${label}`);
  const secret = diffieHellman({ privateKey: ephemeral.private, publicKey: publicKey("x", unb64(recipient)) });
  const info = `slink/wrap/v1/${context}/${ephemeral.public}/${recipient}`;
  const derived = Buffer.from(hkdfSync("sha256", secret, Buffer.alloc(0), info, 32));
  return `${ephemeral.public}.${seal(derived, key, info, label)}`;
}

const domain = (kind) => Buffer.from(`slink/${kind}/v1\0`);
/** A statement over exactly these bytes. */
export const signBytes = (kind, raw, key) => ({
  payload: b64(raw),
  signature: b64(signWith(null, Buffer.concat([domain(kind), Buffer.from(raw)]), key.private)),
});
export const sign = (kind, value, key) => signBytes(kind, Buffer.from(JSON.stringify(value)), key);
export const hash = (signed) => digest(unb64(signed.payload));

/** An envelope sealed under a header of the caller's choosing. */
export function envelope(plain, label, header = {}) {
  const key = material(`content key/${label}`);
  const head = Buffer.concat([
    Buffer.from(header.magic ?? [0x53, 0x4c, 0x49, 0x4e, 0x4b, 0x45, 0, 1]),
    Buffer.from(header.reserved ?? [0, 0, 0, 0]),
    material(`nonce/envelope/${label}`, 12),
  ]);
  const cipher = createCipheriv("aes-256-gcm", key, head.subarray(12));
  cipher.setAAD(head);
  return { key: b64(key), data: Buffer.concat([head, cipher.update(plain), cipher.final(), cipher.getAuthTag()]) };
}

/* -------------------------------------------------------------- accounts */

/** What a vault holds before anything is shared. */
const EMPTY_VAULT = Buffer.from(JSON.stringify({ version: 1, shares: [] }));

// Go writes a state's fields in this order, with `inbox` first when there is
// one. The order is not a rule; it is only what a real event looks like.
const ordered = ({ inbox, ...rest }) => (inbox === undefined ? rest : { inbox, ...rest });

/**
 * A fictional account and everything needed to write its history: the keys,
 * devices by label, and one function for each operation. A chain is
 * `{ events, states }`; every function returns a new one.
 */
export function account(name = "usr_fixture") {
  const rootKey = keyPair("ed", `${name}/root`);
  const recovery = keyPair("x", `${name}/recovery`);
  const vaultKey = (epoch) => material(`${name}/vault key/${epoch}`);
  const context = (epoch) => `${name}/${rootKey.public}/${epoch}`;
  const known = new Map();

  const device = (label, displayName = label) => {
    const signKey = keyPair("ed", `${name}/${label}/sign`);
    const boxKey = keyPair("x", `${name}/${label}/box`);
    const made = { label, name: displayName, sign: signKey, box: boxKey, id: digest(`${signKey.public}.${boxKey.public}`) };
    known.set(made.id, made);
    return made;
  };
  /** The device as a state lists it, with the vault key wrapped for `epoch`. */
  const entry = (d, epoch) => ({
    id: d.id,
    name: d.name,
    sign: d.sign.public,
    box: d.box.public,
    wrap: wrap(d.box.public, vaultKey(epoch), context(epoch), `${name}/${d.label}/${epoch}`),
  });
  const recoveryWrap = (epoch) => wrap(recovery.public, vaultKey(epoch), context(epoch), `${name}/recovery/${epoch}`);
  const recoverySecret = material(`${name}/recovery secret`);
  /** The recovery key a person holds for this account. */
  const recoveryKey = `slr1.${digest(`${name}\0${rootKey.public}`)}.${b64(recoverySecret)}`;
  const inboxKey = (label) => keyPair("x", `${name}/inbox/${label}`);
  const inbox = (label) => inboxKey(label).public;
  const keyOf = (signer) => (signer === "root" ? rootKey : known.get(signer).sign);

  const chain = (states, events) => ({ states, events, state: states.at(-1), head: hash(events.at(-1)) });
  const init = (first) => {
    const state = {
      account: name,
      seq: 0,
      prev: "",
      kind: "init",
      signer: "root",
      root: rootKey.public,
      recovery: recovery.public,
      recovery_box: seal(
        recoverySecret,
        Buffer.concat([rootKey.seed, recovery.seed]),
        `slink/recovery/v1/${name}/${rootKey.public}`,
        `${name}/recovery box`,
      ),
      recovery_wrap: recoveryWrap(1),
      epoch: 1,
      devices: [entry(first, 1)],
    };
    return chain([state], [sign("event", state, rootKey)]);
  };
  /** The next event: the last state with `change`, signed by `by`. */
  const extend = (c, change, by) => {
    const state = ordered({ ...c.state, ...change, seq: c.state.seq + 1, prev: c.head, signer: by === "root" ? "root" : by.id });
    return chain([...c.states, state], [...c.events, sign("event", state, by === "root" ? rootKey : by.sign)]);
  };
  const approve = (c, by, added) =>
    extend(c, { kind: "approve", devices: [...c.state.devices, entry(added, c.state.epoch)] }, by);
  const recover = (c, added) =>
    extend(c, { kind: "recover", devices: [...c.state.devices, entry(added, c.state.epoch)] }, "root");
  const sharing = (c, by, label) => extend(c, { kind: "sharing", inbox: inbox(label) }, by);
  /** `by` removes `removed`; an account that shares names its next inbox key. */
  const revoke = (c, by, removed, label) => {
    const epoch = c.state.epoch + 1;
    return extend(
      c,
      {
        kind: "revoke",
        epoch,
        recovery_wrap: recoveryWrap(epoch),
        devices: c.state.devices.filter((d) => d.id !== removed.id).map((d) => entry(known.get(d.id), epoch)),
        ...(label ? { inbox: inbox(label) } : {}),
      },
      by,
    );
  };
  /**
   * The chain's events with the last one made again. `change` gets a copy of
   * its state and returns the state to sign, or bytes to sign as they are.
   * It is signed by the key that signed the original unless `key` is given.
   */
  const amend = (c, change, { key, kind = "event" } = {}) => {
    const state = change(structuredClone(c.state));
    const signer = key ?? keyOf(c.state.signer);
    const event = Buffer.isBuffer(state) ? signBytes(kind, state, signer) : sign(kind, state, signer);
    return [...c.events.slice(0, -1), event];
  };
  /** The vault statement a client writes for the chain's last state. */
  const vault = (c, by, revision = 1, change = (value) => value, { key, kind = "vault", contents = EMPTY_VAULT } = {}) => {
    const epoch = c.state.epoch;
    const value = change({
      account: name,
      revision,
      head: c.head,
      epoch,
      signer: by === "root" ? "root" : by.id,
      data: seal(vaultKey(epoch), contents, `slink/vault/v1/${context(epoch)}`, `${name}/vault/${epoch}/${revision}`),
    });
    const signer = key ?? (by === "root" ? rootKey : by.sign);
    return Buffer.isBuffer(value) ? signBytes(kind, value, signer) : sign(kind, value, signer);
  };
  return { name, root: rootKey, recovery, recoverySecret, recoveryKey, vaultKey, context, device, entry, recoveryWrap, inbox, inboxKey, keyOf, init, extend, approve, recover, sharing, revoke, amend, vault };
}

/** What a verifier must report of a history it accepts. */
export const summary = (c) => ({
  head: c.head,
  seq: c.state.seq,
  kind: c.state.kind,
  signer: c.state.signer,
  epoch: c.state.epoch,
  inbox: c.state.inbox ?? "",
  devices: c.state.devices.map((d) => ({ id: d.id, name: d.name })),
});

/* ------------------------------------------------------------- envelopes */

function envelopeCases() {
  const cases = [];
  const add = (name, data, key, plain) =>
    cases.push({
      name,
      envelope: Buffer.from(data).toString("base64"),
      key,
      expect: plain === undefined ? "refused" : "opens",
      ...(plain === undefined ? {} : { plaintext: Buffer.from(plain).toString("base64") }),
    });
  const flipped = (data, at) => {
    const out = Buffer.from(data);
    out[at < 0 ? out.length + at : at] ^= 1;
    return out;
  };
  const recorded = {};
  for (const [source, file] of [["Web Crypto", "webcrypto.json"], ["Go", "go.json"]]) {
    const fixture = JSON.parse(readFileSync(path.join(root, "testdata/encryption", file), "utf8"));
    const data = Buffer.from(fixture.envelope, "base64");
    recorded[source] = { data, key: fixture.key };
    add(`an envelope made by ${source}`, data, fixture.key, fixture.plaintext);
    for (const [where, at] of [
      ["the magic", 0],
      ["the magic's last letter", 5],
      ["the version's high byte", 6],
      ["the version", 7],
      ["the first reserved byte", 8],
      ["the last reserved byte", 11],
      ["the nonce's first byte", 12],
      ["the nonce's last byte", 23],
      ["the first byte of the document", 24],
      ["the last byte of the document", -17],
      ["the tag's first byte", -16],
      ["the tag's last byte", -1],
    ]) {
      add(`${source}: one bit changed in ${where}`, flipped(data, at), fixture.key);
    }
    add(`${source}: the last byte missing`, data.subarray(0, -1), fixture.key);
    add(`${source}: the header and nothing else`, data.subarray(0, 24), fixture.key);
    add(`${source}: one byte short of a header and a tag`, data.subarray(0, 39), fixture.key);
    add(`${source}: the header and the tag without the document`, Buffer.concat([data.subarray(0, 24), data.subarray(-16)]), fixture.key);
    add(`${source}: one byte added`, Buffer.concat([data, Buffer.from([0])]), fixture.key);
    add(`${source}: no bytes at all`, Buffer.alloc(0), fixture.key);
  }
  const { data, key } = recorded.Go;
  add("the other envelope's key", data, recorded["Web Crypto"].key);
  if (!key.includes("-")) throw new Error("the Go fixture's key no longer has a character the two alphabets differ on");
  const lastBits = key.slice(0, -1) + "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_"["ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_".indexOf(key.at(-1)) + 1];
  for (const [what, bad] of [
    ["with padding", `${key}=`],
    ["followed by a line break", `${key}\n`],
    ["after a space", ` ${key}`],
    ["one character short", key.slice(0, -1)],
    ["one character long", `${key}A`],
    ["in the other base64 alphabet", key.replaceAll("-", "+").replaceAll("_", "/")],
    ["with stray bits in its last character", lastBits],
    ["that is empty", ""],
  ]) {
    add(`a key ${what}`, data, bad);
  }

  const made = (name, plain, label, header) => {
    const sealed = envelope(plain, label, header);
    add(name, sealed.data, sealed.key, header ? undefined : plain);
  };
  made("an empty document", Buffer.alloc(0), "empty");
  made("a document of one byte", Buffer.from([0x7b]), "one byte");
  made("a document of every byte value", Buffer.from(Array.from({ length: 256 }, (_, i) => i)), "every byte");
  // These authenticate: the header is the associated data they were sealed
  // under. Only the check of the header itself refuses them.
  const text = Buffer.from("sealed under a header no reader supports");
  made("version 2, sealed and authentic", text, "version 2", { magic: [0x53, 0x4c, 0x49, 0x4e, 0x4b, 0x45, 0, 2] });
  made("version 257, sealed and authentic", text, "version 257", { magic: [0x53, 0x4c, 0x49, 0x4e, 0x4b, 0x45, 1, 1] });
  made("another magic, sealed and authentic", text, "magic", { magic: [0x53, 0x4c, 0x49, 0x4e, 0x4b, 0x46, 0, 1] });
  made("a reserved byte set, sealed and authentic", text, "reserved", { reserved: [0, 0, 0, 1] });
  made("the first reserved byte set, sealed and authentic", text, "reserved first", { reserved: [1, 0, 0, 0] });
  return cases;
}

/* ------------------------------------------------------------ statements */

const KINDS = ["event", "vault", "request", "recovery-check", "named-invite", "recipient-claim", "named-grant"];
const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const otherAlphabet = (text) => text.replaceAll("-", "+").replaceAll("_", "/");
/** The same length, with the bits a decoder must find zero set to one. */
const strayBits = (text) => text.slice(0, -1) + ALPHABET[ALPHABET.indexOf(text.at(-1)) + 1];

function statementCases() {
  const cases = [];
  const key = keyPair("ed", "statement/key");
  const other = keyPair("ed", "statement/other key");
  const add = (name, kind, pub, signed, expect, why) =>
    cases.push({ name, kind, public: pub, payload: signed.payload, signature: signed.signature, expect, ...(why ? { why } : {}) });
  const value = { statement: "fixture" };

  for (const kind of KINDS) add(`a statement signed as ${kind}`, kind, key.public, sign(kind, value, key), "verified");
  for (const [made, offered] of [["event", "vault"], ["vault", "event"], ["request", "recovery-check"], ["named-invite", "named-grant"], ["named-grant", "recipient-claim"], ["recipient-claim", "event"]]) {
    add(`a statement signed as ${made} and offered as ${offered}`, offered, key.public, sign(made, value, key), "refused");
  }
  const good = sign("event", value, key);
  add("another key's signature", "event", other.public, good, "refused");
  add("a payload changed after signing", "event", key.public, { ...good, payload: b64(JSON.stringify({ statement: "fixturf" })) }, "refused");
  const bit = unb64(good.signature);
  bit[40] ^= 1;
  add("a signature with one bit changed", "event", key.public, { ...good, signature: b64(bit) }, "refused");
  add("an empty payload, signed", "event", key.public, signBytes("event", Buffer.alloc(0), key), "verified");
  add("a payload that is not text, signed", "event", key.public, signBytes("event", Buffer.from([0, 0xff, 0xfe, 0x80]), key), "verified");

  // One spelling of the bytes is accepted. Each of these decodes, in a
  // lenient decoder, to bytes the signature is good for.
  const spelt = signBytes("event", Buffer.from([0xfb, 0xff, 0xfe, 0xfb, 0xff]), key);
  if (!/[-_]/.test(spelt.payload) || spelt.payload.length % 4 === 0) throw new Error("the spelling payload lost its point");
  add("a payload with padding", "event", key.public, { ...spelt, payload: `${spelt.payload}=` }, "refused");
  add("a payload in the other base64 alphabet", "event", key.public, { ...spelt, payload: otherAlphabet(spelt.payload) }, "refused");
  add("a payload with stray bits in its last character", "event", key.public, { ...spelt, payload: strayBits(spelt.payload) }, "refused");
  add("a payload with a line break in it", "event", key.public, { ...spelt, payload: `${spelt.payload.slice(0, 4)}\n${spelt.payload.slice(4)}` }, "refused");
  add("a payload that ends with a line break", "event", key.public, { ...spelt, payload: `${spelt.payload}\n` }, "refused");
  add("a payload with a space in it", "event", key.public, { ...spelt, payload: `${spelt.payload.slice(0, 4)} ${spelt.payload.slice(4)}` }, "refused");

  // A signature and a key that hold a character the two alphabets differ on.
  let marked, signer;
  for (let i = 0; !marked; i++) {
    const candidate = sign("event", { statement: "fixture", n: i }, key);
    if (/[-_]/.test(candidate.signature)) marked = candidate;
  }
  for (let i = 0; !signer; i++) {
    const candidate = keyPair("ed", `statement/marked key/${i}`);
    if (/[-_]/.test(candidate.public)) signer = candidate;
  }
  const raw = unb64(marked.signature);
  add("a signature with padding", "event", key.public, { ...marked, signature: `${marked.signature}==` }, "refused");
  add("a signature in the other base64 alphabet", "event", key.public, { ...marked, signature: otherAlphabet(marked.signature) }, "refused");
  add("a signature with stray bits in its last character", "event", key.public, { ...marked, signature: strayBits(marked.signature) }, "refused");
  add("a signature one byte short", "event", key.public, { ...marked, signature: b64(raw.subarray(0, 63)) }, "refused");
  add("a signature one byte long", "event", key.public, { ...marked, signature: b64(Buffer.concat([raw, Buffer.from([0])])) }, "refused");
  add("no signature", "event", key.public, { ...marked, signature: "" }, "refused");
  add("a signature followed by a line break", "event", key.public, { ...marked, signature: `${marked.signature}\n` }, "refused");

  const own = sign("event", value, signer);
  add("a key that holds both special characters' alphabet", "event", signer.public, own, "verified");
  add("a key with padding", "event", `${signer.public}=`, own, "refused");
  add("a key in the other base64 alphabet", "event", otherAlphabet(signer.public), own, "refused");
  add("a key with stray bits in its last character", "event", strayBits(signer.public), own, "refused");
  add("a key one byte short", "event", b64(unb64(signer.public).subarray(0, 31)), own, "refused");
  add("a key one byte long", "event", b64(Buffer.concat([unb64(signer.public), Buffer.from([0])])), own, "refused");
  add("no key", "event", "", own, "refused");

  // Where Ed25519 libraries are known to part ways. Every implementation
  // here decides these the same way today; a library that decides one
  // differently fails its case.
  const order = (1n << 252n) + 27742317777372353535851937790883648493n;
  const scalar = BigInt(`0x${Buffer.from(raw.subarray(32)).reverse().toString("hex")}`) + order;
  const high = Buffer.from(scalar.toString(16).padStart(64, "0"), "hex").reverse();
  add(
    "a signature whose second half has the group order added",
    "event",
    key.public,
    { ...marked, signature: b64(Buffer.concat([raw.subarray(0, 32), high])) },
    "refused",
    "The same point by another number: accepting it would give every statement a second signature.",
  );
  // A key of small order. Left to itself every Ed25519 library here verifies
  // under one, and for the identity point one signature fits every payload,
  // so each implementation refuses these keys before it asks its library.
  // Each case is on a payload the library would have taken.
  const weak = "A signing key of small order proves nothing about who signed: for the identity point one signature fits every payload. The libraries verify under such a key, so every implementation refuses it itself.";
  const field = (1n << 255n) - 19n;
  const bytesOf = (number) => Buffer.from(number.toString(16).padStart(64, "0"), "hex").reverse();
  const empty = b64(Buffer.concat([bytesOf(1n), Buffer.alloc(32)]));
  for (const [what, y] of [
    ["the identity point", 1n],
    ["the identity point spelt past the field's size", field + 1n],
    ["the point of order two", field - 1n],
    ["a point of order four", 0n],
    ["a point of order four spelt past the field's size", field],
    ["a point of order eight", 0x7a03ac9277fdc74ec6cc392cfa53202a0f67100d760b3cba4fd84d3d706a17c7n],
    ["another point of order eight", 0x05fc536d880238b13933c6d305acdfd5f098eff289f4c345b027b2c28f95e826n],
  ]) {
    for (const sign of [0n, 1n]) {
      const point = bytesOf(y | (sign << 255n));
      let payload;
      for (let n = 0; n < 400 && !payload; n++) {
        const candidate = Buffer.from(JSON.stringify({ statement: "fixture", n }));
        if (verifyWith(null, Buffer.concat([domain("event"), candidate]), publicKey("ed", point), unb64(empty))) payload = candidate;
      }
      if (!payload) throw new Error(`node:crypto takes no payload under ${what}`);
      add(`${what}${sign ? " with the sign bit set" : ""} as a key, on a payload the empty signature fits`, "event", b64(point), { payload: b64(payload), signature: empty }, "refused", weak);
    }
  }
  // The point in a signature is compared as bytes. This one is made with the
  // key: the identity as the point, spelt past the field's size, and the
  // number that goes with it. Spelt the usual way it verifies.
  const scalar512 = (bytes) => BigInt(`0x${Buffer.from(createHash("sha512").update(bytes).digest()).reverse().toString("hex")}`);
  const secret = createHash("sha512").update(key.seed).digest().subarray(0, 32);
  secret[0] &= 248;
  secret[31] = (secret[31] & 127) | 64;
  const secretScalar = BigInt(`0x${Buffer.from(secret).reverse().toString("hex")}`);
  const withPoint = (point) => {
    const message = Buffer.concat([domain("event"), unb64(good.payload)]);
    const k = scalar512(Buffer.concat([point, unb64(key.public), message])) % order;
    return b64(Buffer.concat([point, bytesOf((k * secretScalar) % order)]));
  };
  if (!verifyWith(null, Buffer.concat([domain("event"), unb64(good.payload)]), key.private, unb64(withPoint(bytesOf(1n))))) {
    throw new Error("the signature made around the identity point does not verify");
  }
  add(
    "a signature made with the key around a point spelt past the field's size",
    "event",
    key.public,
    { payload: good.payload, signature: withPoint(bytesOf(field + 1n)) },
    "refused",
    "The same signature with the point spelt the usual way verifies. A library that compared points and not bytes would take both, and every statement would have a second signature.",
  );
  return cases;
}

/* ------------------------------------------------------------- histories */

/** Collects cases and stores each signed statement once, under a short id. */
function collector() {
  const signed = {};
  const cases = [];
  const ref = (statement) => {
    const id = createHash("sha256").update(`${statement.payload}.${statement.signature}`).digest("hex").slice(0, 12);
    if (signed[id] && (signed[id].payload !== statement.payload || signed[id].signature !== statement.signature)) {
      throw new Error(`two statements share the id ${id}`);
    }
    signed[id] = statement;
    return id;
  };
  const push = (item) => {
    if (cases.some((c) => c.name === item.name)) throw new Error(`two cases are named ${item.name}`);
    cases.push(item);
  };
  const record = (vault) => (vault ? { vault: ref(vault) } : {});
  return {
    signed,
    cases,
    /** A chain every implementation accepts, and the state it must report. */
    valid(name, c, { vault, revision, account: of } = {}) {
      // Spelt as a client spells it, in every statement: the Go CLI and the
      // Rust crate must then write the same bytes for what they read.
      const client = c.events.every((event) => asWritten(event, writtenState)) && (!vault || asWritten(vault, writtenVault));
      push({ name, account: of ?? c.state.account, events: c.events.map(ref), ...record(vault), expect: "valid", ...(client ? { client } : {}), state: { ...summary(c), ...(vault ? { revision } : {}) } });
    },
    /** Events every implementation refuses; `rule` is what the Rust crate names. */
    invalid(name, rule, events, { vault, account: of = "usr_fixture" } = {}) {
      push({ name, account: of, events: events.map(ref), ...record(vault), expect: "invalid", rule });
    },
    /** A known difference: one verdict for each implementation, and why. */
    differs(name, expect, why, events, { vault, rule, state, account: of = "usr_fixture" } = {}) {
      // `state` is what the unchanged chain reports; these events end in other
      // bytes, so in another head.
      const head = hash(events.at(-1));
      const read = state.js ? { js: { ...state.js, head }, go: { ...state.go, head } } : { ...state, head };
      push({ name, account: of, events: events.map(ref), ...record(vault), expect, ...(rule ? { rule } : {}), state: read, why });
    },
  };
}

/** The state as JSON text with one field's value written exactly as given. */
const rewritten = (state, field, token) => {
  const marker = "\u0000rewritten\u0000";
  const text = JSON.stringify({ ...state, [field]: marker });
  return Buffer.from(text.replace(JSON.stringify(marker), token));
};
/** The state as JSON text with one field's name written exactly as given. */
const renamed = (state, field, name) => Buffer.from(JSON.stringify(state).replace(`"${field}":`, `${name}:`));
const backslash = String.fromCharCode(0x5c);

function historyCases() {
  const out = collector();
  const { valid, invalid, differs } = out;
  const A = account("usr_fixture");
  const laptop = A.device("laptop", "Laptop");
  const phone = A.device("phone", "Phone");
  const tablet = A.device("tablet", "Tablet");
  const desktop = A.device("desktop", "Desktop");

  const first = A.init(laptop);
  const shared = A.sharing(first, laptop, "a");
  const approved = A.approve(first, laptop, phone);
  const recovered = A.recover(first, phone);
  const revoked = A.revoke(approved, laptop, phone);
  const three = A.approve(approved, phone, tablet);
  const sharedTwo = A.approve(shared, laptop, phone);
  const rotated = A.revoke(sharedTwo, laptop, phone, "b");
  const rotatedTwo = A.approve(rotated, laptop, tablet);
  const rotatedTwice = A.revoke(rotatedTwo, laptop, tablet, "c");
  const before = (c) => ({ states: c.states.slice(0, -1), events: c.events.slice(0, -1), state: c.states.at(-2), head: hash(c.events.at(-2)) });

  /* What a client writes. */
  valid("the first event alone", first);
  valid("sharing turned on", shared);
  valid("a device approves a second", approved);
  valid("the recovery key enrolls a second device", recovered);
  valid("the second device approves a third", three);
  valid("a device is revoked", revoked);
  valid("the newer device revokes the first", A.revoke(approved, phone, laptop));
  valid("one of three devices is revoked by another", A.revoke(three, tablet, laptop));
  valid("sharing turned on by the second device", A.sharing(approved, phone, "a"));
  valid("a revocation gives the account a new incoming-share key", rotated);
  valid("a device is approved after the key was rotated", rotatedTwo);
  valid("a second revocation takes a third incoming-share key", rotatedTwice);
  valid("the recovery key enrolls a device after a revocation", A.recover(revoked, tablet));
  valid("a revoked device is approved again", A.approve(revoked, laptop, phone));
  valid("sharing turned on after a revocation", A.sharing(revoked, laptop, "a"));

  /* What a client need not write but every verifier reads the same way. */
  const amended = (c, change, options) => {
    const events = A.amend(c, change, options);
    const state = change(structuredClone(c.state));
    return { events, states: [...c.states.slice(0, -1), Buffer.isBuffer(state) ? c.state : state], state: Buffer.isBuffer(state) ? c.state : state, head: hash(events.at(-1)) };
  };
  valid(
    "fields no verifier knows are carried and ignored",
    amended(first, (s) => ({ ...s, note: "unknown", future: { list: [1, { nested: null }], flag: true }, devices: s.devices.map((d) => ({ ...d, platform: "unknown" })) })),
  );
  valid("the fields in another order", amended(first, (s) => Object.fromEntries(Object.entries(s).reverse())));
  valid("white space around and inside the payload", amended(first, (s) => Buffer.from(`\n  ${JSON.stringify(s, null, 2)}\r\n\t `)));
  valid(
    "a field's name written with an escape",
    amended(first, (s) => renamed(s, "account", `"${backslash}u0061ccount"`)),
  );
  valid("an incoming-share key that is null", amended(first, (s) => rewritten(s, "inbox", "null")));
  valid("an incoming-share key that is empty", amended(first, (s) => ({ ...s, inbox: "" })));
  const named = (label, name) => {
    const d = A.device(label, name);
    return A.init(d);
  };
  valid("a device with no name", named("unnamed", ""));
  valid("a device name of 80 letters", named("name 80", "n".repeat(80)));
  valid("a device name of 40 two-byte characters", named("name 40x2", String.fromCodePoint(0xe9).repeat(40)));
  valid("a device name of 20 four-byte characters", named("name 20x4", String.fromCodePoint(0x1f4bb).repeat(20)));
  valid("a device name with a quote, a backslash and a control character", named("name escapes", `a"b${backslash}c\u0001d\n`));

  /* The history as a whole. */
  invalid("no events", "length", []);
  invalid("the first event twice", "sequence", [first.events[0], first.events[0]]);
  invalid("two events in the wrong order", "sequence", [approved.events[1], approved.events[0]]);
  invalid("a history that starts at its second event", "sequence", [approved.events[1]]);
  invalid("verified for another account", "account", first.events, { account: "usr_other" });
  invalid("verified for an account whose name differs in case", "account", first.events, { account: "USR_FIXTURE" });

  /* The signed object. */
  const e0 = first.events[0];
  invalid("a payload with padding", "signed", [{ ...e0, payload: `${e0.payload}=` }]);
  invalid("a payload with stray bits in its last character", "signed", [{ ...e0, payload: e0.payload.length % 4 ? strayBits(e0.payload) : `${e0.payload}A` }]);
  // Cut at a multiple of four characters, so what is left is base64url and
  // only the JSON inside is incomplete.
  invalid("a payload cut short", "payload", [{ ...e0, payload: e0.payload.slice(0, Math.floor(e0.payload.length / 4) * 4 - 4) }]);
  invalid("a signature one byte short", "signed", [{ ...e0, signature: b64(unb64(e0.signature).subarray(0, 63)) }]);
  invalid("a signature with padding", "signed", [{ ...e0, signature: `${e0.signature}==` }]);
  invalid("a signature made as a vault statement", "signature", A.amend(first, (s) => s, { kind: "vault" }));
  invalid(
    "a device renamed in the payload after signing",
    "signature",
    [{ ...e0, payload: b64(JSON.stringify(first.state).replace('"Laptop"', '"Laptoq"')) }],
  );
  invalid("the second event's signature on the first", "signature", [{ ...e0, signature: approved.events[1].signature }]);

  /* The payload as JSON. */
  for (const [what, text] of [
    ["is not JSON", "not json"],
    ["is empty", ""],
    ["is a JSON array", "[]"],
    ["is null", "null"],
    ["is a JSON string", '"init"'],
    ["is an empty object", "{}"],
  ]) {
    invalid(`a payload that ${what}`, "payload", A.amend(first, () => Buffer.from(text)));
  }
  invalid("text after the payload's JSON", "payload", A.amend(first, (s) => Buffer.from(`${JSON.stringify(s)}x`)));
  invalid("a second JSON value after the payload", "payload", A.amend(first, (s) => Buffer.from(`${JSON.stringify(s)}{}`)));
  invalid("a payload cut off before its last brace", "payload", A.amend(first, (s) => Buffer.from(JSON.stringify(s).slice(0, -1))));
  // serde can read a struct from an array of its fields in order. Nothing
  // else reads a statement that way, so neither may the Rust crate.
  invalid("the state as an array of its fields", "payload", A.amend(first, (s) => Buffer.from(JSON.stringify(Object.values(s)))));
  invalid(
    "a device as an array of its fields",
    "payload",
    A.amend(first, (s) => ({ ...s, devices: s.devices.map((d) => Object.values(d)) })),
  );
  for (const [what, field, token] of [
    ["a sequence number in quotes", "seq", '"0"'],
    ["an epoch in quotes", "epoch", '"1"'],
    ["an epoch that is true", "epoch", "true"],
    ["a negative epoch", "epoch", "-1"],
    ["an epoch with a fraction", "epoch", "1.5"],
    ["a kind that is a number", "kind", "1"],
    ["a signer that is a list", "signer", '["root"]'],
    ["a root key that is a number", "root", "7"],
    ["devices as an object", "devices", "{}"],
    ["devices as text", "devices", '"none"'],
    ["a device that is null", "devices", "[null]"],
    ["a device that is text", "devices", '["laptop"]'],
    ["an incoming-share key that is a number", "inbox", "7"],
    ["an incoming-share key that is an object", "inbox", "{}"],
  ]) {
    invalid(what, "payload", A.amend(first, (s) => rewritten(s, field, token)));
  }
  invalid("a kind written in capitals", "kind", A.amend(first, (s) => ({ ...s, kind: "INIT" })));

  /* The fields every event has. */
  invalid("an event for another account", "account", A.amend(first, (s) => ({ ...s, account: "usr_other" })));
  invalid("a second event for another account", "account", A.amend(approved, (s) => ({ ...s, account: "usr_other" })));
  invalid("a first event numbered one", "sequence", A.amend(first, (s) => ({ ...s, seq: 1 })));
  invalid("a second event numbered two", "sequence", A.amend(approved, (s) => ({ ...s, seq: 2 })));
  invalid("a second event numbered zero", "sequence", A.amend(approved, (s) => ({ ...s, seq: 0 })));
  invalid("a first event that names a predecessor", "previous", A.amend(first, (s) => ({ ...s, prev: first.head })));
  invalid("a second event with no predecessor", "previous", A.amend(approved, (s) => ({ ...s, prev: "" })));
  invalid("a second event that follows another history", "previous", A.amend(approved, (s) => ({ ...s, prev: shared.head })));
  invalid("a third event that follows the first", "previous", A.amend(three, (s) => ({ ...s, prev: first.head })));
  invalid("an epoch of zero", "epoch", A.amend(first, (s) => ({ ...s, epoch: 0 })));
  invalid("no devices", "devices", A.amend(first, (s) => ({ ...s, devices: [] })));

  /* Keys. */
  const short = b64(material("short key", 31));
  const long = b64(material("long key", 33));
  const withID = (d) => ({ ...d, id: digest(`${d.sign}.${d.box}`) });
  invalid("a root key of 31 bytes", "key", A.amend(first, (s) => ({ ...s, root: short })));
  invalid("a root key with padding", "key", A.amend(first, (s) => ({ ...s, root: `${s.root}=` })));
  invalid("a recovery key of 33 bytes", "key", A.amend(first, (s) => ({ ...s, recovery: long })));
  invalid("a recovery key with stray bits in its last character", "key", A.amend(first, (s) => ({ ...s, recovery: strayBits(s.recovery) })));
  invalid("an incoming-share key of 31 bytes", "key", A.amend(shared, (s) => ({ ...s, inbox: short })));
  invalid("an incoming-share key that is a word", "key", A.amend(shared, (s) => ({ ...s, inbox: "none" })));
  invalid("a device's signing key of 31 bytes", "key", A.amend(first, (s) => ({ ...s, devices: [withID({ ...s.devices[0], sign: short })] })));
  invalid("a device's agreement key of 33 bytes", "key", A.amend(first, (s) => ({ ...s, devices: [withID({ ...s.devices[0], box: long })] })));
  invalid(
    "a device's agreement key in the other base64 alphabet",
    "key",
    A.amend(first, (s) => ({ ...s, devices: [withID({ ...s.devices[0], box: `${"+".repeat(42)}A` })] })),
  );

  /* Wrappers: their shape only. */
  const lenient = "Go checks only how long a wrapper is. JavaScript and Rust also split it and decode both halves.";
  const wrapper = first.state.devices[0].wrap;
  const onDevice = (wrap) => A.amend(first, (s) => ({ ...s, devices: [{ ...s.devices[0], wrap }] }));
  invalid("a device's wrapper one character short", "wrapper", onDevice(wrapper.slice(0, -1)));
  invalid("a device's wrapper one character long", "wrapper", onDevice(`${wrapper}A`));
  invalid("a device with no wrapper", "wrapper", onDevice(""));
  invalid("a recovery wrapper one character short", "wrapper", A.amend(first, (s) => ({ ...s, recovery_wrap: s.recovery_wrap.slice(0, -1) })));
  invalid("a recovery box one character short", "wrapper", A.amend(first, (s) => ({ ...s, recovery_box: s.recovery_box.slice(0, -1) })));
  invalid("a recovery box one character long", "wrapper", A.amend(first, (s) => ({ ...s, recovery_box: `${s.recovery_box}A` })));
  const goOnly = { js: "invalid", go: "valid", rust: "invalid" };
  const stateOf = (c) => summary(c);
  differs("a device's wrapper that is 124 letters", goOnly, lenient, onDevice("A".repeat(124)), { rule: "wrapper", state: stateOf(first) });
  differs(
    "a device's wrapper with its dot one place late",
    goOnly,
    lenient,
    onDevice(`${wrapper.slice(0, 43)}A.${wrapper.slice(45)}`),
    { rule: "wrapper", state: stateOf(first) },
  );
  differs(
    "a recovery wrapper whose first half has stray bits",
    goOnly,
    lenient,
    A.amend(first, (s) => ({ ...s, recovery_wrap: `${strayBits(s.recovery_wrap.slice(0, 43))}${s.recovery_wrap.slice(43)}` })),
    { rule: "wrapper", state: stateOf(first) },
  );
  differs(
    "a recovery box of the right length in the other base64 alphabet",
    goOnly,
    lenient,
    A.amend(first, (s) => ({ ...s, recovery_box: "+".repeat(123) })),
    { rule: "wrapper", state: stateOf(first) },
  );

  /* Devices. */
  invalid("a device under another device's id", "device", A.amend(first, (s) => ({ ...s, devices: [{ ...s.devices[0], id: phone.id }] })));
  invalid("a device under an id that is a word", "device", A.amend(first, (s) => ({ ...s, devices: [{ ...s.devices[0], id: "laptop" }] })));
  invalid("the same device twice in the first event", "device", A.amend(first, (s) => ({ ...s, devices: [s.devices[0], s.devices[0]] })));
  invalid("the same device twice in an approval", "device", A.amend(approved, (s) => ({ ...s, devices: [...s.devices, s.devices[1]] })));
  const tooLong = (label, name) => A.amend(named(label, "n"), (s) => ({ ...s, devices: [{ ...s.devices[0], name }] }));
  invalid("a device name of 81 letters", "device", tooLong("name 81", "n".repeat(81)));
  invalid("a device name of 27 three-byte characters", "device", tooLong("name 27x3", String.fromCodePoint(0x20ac).repeat(27)));
  invalid("a device name of 41 two-byte characters", "device", tooLong("name 41x2", String.fromCodePoint(0xe9).repeat(41)));

  /* The first event. */
  invalid("a first event that is an approval", "initial", A.amend(first, (s) => ({ ...s, kind: "approve" })));
  invalid("a first event that is a recovery", "initial", A.amend(first, (s) => ({ ...s, kind: "recover" })));
  invalid("a first event in a device's name", "initial", A.amend(first, (s) => ({ ...s, signer: laptop.id }), { key: laptop.sign }));
  invalid("a first event at epoch two", "initial", A.amend(first, (s) => ({ ...s, epoch: 2 })));
  invalid("a first event with two devices", "initial", A.amend(first, (s) => ({ ...s, devices: [...s.devices, A.entry(phone, 1)] })));
  invalid("a first event signed by its device", "signature", A.amend(first, (s) => s, { key: laptop.sign }));
  invalid("a first event signed by another account's root key", "signature", A.amend(first, (s) => s, { key: account("usr_other").root }));
  invalid("an operation no verifier knows", "kind", A.amend(approved, (s) => ({ ...s, kind: "rename" })));
  invalid("a second first event", "kind", A.amend(approved, (s) => ({ ...s, kind: "init" })));

  /* What never changes. */
  const stranger = account("usr_other");
  const strangerFirst = stranger.init(stranger.device("laptop", "Laptop"));
  invalid("the root key replaced", "recovery", A.amend(approved, (s) => ({ ...s, root: stranger.root.public })));
  invalid("the recovery key replaced", "recovery", A.amend(approved, (s) => ({ ...s, recovery: stranger.recovery.public })));
  invalid("the recovery box replaced", "recovery", A.amend(approved, (s) => ({ ...s, recovery_box: strangerFirst.state.recovery_box })));
  invalid("the root key replaced by the recovery key in a recovery", "recovery", A.amend(recovered, (s) => ({ ...s, root: stranger.root.public })));

  /* Who may sign. */
  invalid("signed by a device the history never held", "signer", A.amend(approved, (s) => ({ ...s, signer: tablet.id }), { key: tablet.sign }));
  invalid("signed by the device it adds", "signer", A.amend(approved, (s) => ({ ...s, signer: phone.id }), { key: phone.sign }));
  invalid("an approval in the root key's name", "signer", A.amend(approved, (s) => ({ ...s, signer: "root" }), { key: A.root }));
  invalid("sharing turned on in the root key's name", "signer", A.amend(shared, (s) => ({ ...s, signer: "root" }), { key: A.root }));
  invalid("a revocation in the root key's name", "signer", A.amend(revoked, (s) => ({ ...s, signer: "root" }), { key: A.root }));
  invalid("a recovery in a device's name", "signer", A.amend(recovered, (s) => ({ ...s, signer: laptop.id }), { key: laptop.sign }));
  invalid("signed by a device that was revoked", "signer", A.approve(revoked, phone, tablet).events);
  invalid("a revocation that removes its signer", "signer", A.revoke(approved, laptop, laptop).events);
  invalid("an approval signed by the root key in a device's name", "signature", A.amend(approved, (s) => s, { key: A.root }));
  invalid("an approval signed by another device's key", "signature", A.amend(three, (s) => s, { key: laptop.sign }));
  invalid("a recovery signed by a device", "signature", A.amend(recovered, (s) => s, { key: laptop.sign }));
  invalid("an approval signed as a vault statement", "signature", A.amend(approved, (s) => s, { kind: "vault" }));

  /* Devices already in the history. */
  const firstDevice = (change) => (s) => ({ ...s, devices: [{ ...s.devices[0], ...change }, ...s.devices.slice(1)] });
  invalid("an approval that renames a device", "device-changed", A.amend(approved, firstDevice({ name: "Renamed" })));
  invalid("an approval that replaces a device's wrapper", "device-changed", A.amend(approved, firstDevice({ wrap: A.entry(laptop, 2).wrap })));
  invalid("a recovery that replaces a device's wrapper", "device-changed", A.amend(recovered, firstDevice({ wrap: A.entry(laptop, 2).wrap })));
  invalid("sharing turned on with a device renamed", "device-changed", A.amend(shared, firstDevice({ name: "Renamed" })));
  invalid("sharing turned on with a wrapper replaced", "device-changed", A.amend(shared, firstDevice({ wrap: A.entry(laptop, 2).wrap })));
  invalid("a revocation that renames the device that stays", "device-changed", A.amend(revoked, firstDevice({ name: "Renamed" })));

  /* The incoming-share key. */
  invalid("an approval that adds an incoming-share key", "inbox", A.amend(approved, (s) => ({ ...s, inbox: A.inbox("a") })));
  invalid("an approval that drops the incoming-share key", "inbox", A.amend(sharedTwo, ({ inbox, ...s }) => s));
  invalid("an approval that changes the incoming-share key", "inbox", A.amend(sharedTwo, (s) => ({ ...s, inbox: A.inbox("b") })));
  invalid("a recovery that changes the incoming-share key", "inbox", A.amend(A.recover(shared, phone), (s) => ({ ...s, inbox: A.inbox("b") })));
  invalid("sharing turned on twice", "inbox", A.sharing(shared, laptop, "b").events);
  invalid("sharing turned on with the key it already has", "inbox", A.sharing(shared, laptop, "a").events);
  invalid("sharing turned on without a key", "inbox", A.amend(shared, ({ inbox, ...s }) => s));
  invalid("a revocation that keeps the incoming-share key", "inbox", A.amend(rotated, (s) => ({ ...s, inbox: A.inbox("a") })));
  invalid("a revocation that drops the incoming-share key", "inbox", A.amend(rotated, ({ inbox, ...s }) => s));
  invalid("a revocation that turns sharing on", "inbox", A.revoke(approved, laptop, phone, "a").events);
  invalid("a retired incoming-share key comes back", "inbox-reuse", A.revoke(rotatedTwo, laptop, tablet, "a").events);
  const twiceMore = A.revoke(A.approve(rotatedTwice, laptop, phone), laptop, phone, "b");
  invalid("a key retired two rotations ago comes back", "inbox-reuse", twiceMore.events);

  /* What each operation may change. */
  invalid("sharing turned on with a device added", "operation", A.amend(shared, (s) => ({ ...s, devices: [...s.devices, A.entry(phone, 1)] })));
  invalid("sharing turned on with a device removed", "operation", A.amend(A.sharing(approved, laptop, "a"), (s) => ({ ...s, devices: [s.devices[0]] })));
  invalid("sharing turned on at the next epoch", "operation", A.amend(shared, (s) => ({ ...s, epoch: 2 })));
  invalid("sharing turned on with a new recovery wrapper", "operation", A.amend(shared, (s) => ({ ...s, recovery_wrap: A.recoveryWrap(2) })));
  invalid("an approval that adds two devices", "operation", A.amend(approved, (s) => ({ ...s, devices: [...s.devices, A.entry(tablet, 1)] })));
  invalid("an approval that adds no device", "operation", A.amend(approved, (s) => ({ ...s, devices: [s.devices[0]] })));
  invalid("an approval that swaps a device for another", "operation", A.amend(three, (s) => ({ ...s, devices: [s.devices[0], s.devices[2]] })));
  invalid("an approval that only removes a device", "operation", A.amend(three, (s) => ({ ...s, devices: [s.devices[0]] })));
  invalid("an approval at the next epoch", "operation", A.amend(approved, (s) => ({ ...s, epoch: 2 })));
  invalid("an approval at an earlier epoch", "operation", A.amend(A.approve(revoked, laptop, phone), (s) => ({ ...s, epoch: 1 })));
  invalid("an approval with a new recovery wrapper", "operation", A.amend(approved, (s) => ({ ...s, recovery_wrap: A.recoveryWrap(2) })));
  invalid("a recovery that adds no device", "operation", A.amend(recovered, (s) => ({ ...s, devices: [s.devices[0]] })));
  invalid("a recovery that adds two devices", "operation", A.amend(recovered, (s) => ({ ...s, devices: [...s.devices, A.entry(tablet, 1)] })));
  invalid("a recovery at the next epoch", "operation", A.amend(recovered, (s) => ({ ...s, epoch: 2 })));
  const rewrapped = (c, ...kept) => kept.map((d) => A.entry(d, c.state.epoch));
  invalid("a revocation that removes no device", "operation", A.amend(revoked, (s) => ({ ...s, devices: rewrapped(revoked, laptop, phone) })));
  invalid("a revocation that removes two devices", "operation", A.amend(A.revoke(three, laptop, phone), (s) => ({ ...s, devices: [s.devices[0]] })));
  invalid("a revocation that swaps a device for another", "operation", A.amend(revoked, (s) => ({ ...s, devices: rewrapped(revoked, laptop, tablet) })));
  invalid("a revocation at the same epoch", "operation", A.amend(revoked, (s) => ({ ...s, epoch: 1 })));
  invalid("a revocation that skips an epoch", "operation", A.amend(revoked, (s) => ({ ...s, epoch: 3 })));
  invalid("a revocation that keeps the recovery wrapper", "rotation", A.amend(revoked, (s) => ({ ...s, recovery_wrap: approved.state.recovery_wrap })));
  invalid("a revocation that keeps the remaining device's wrapper", "rotation", A.amend(revoked, firstDevice({ wrap: approved.state.devices[0].wrap })));
  invalid(
    "a revocation that keeps one of two remaining wrappers",
    "rotation",
    A.amend(A.revoke(three, laptop, phone), (s) => ({ ...s, devices: [s.devices[0], { ...s.devices[1], wrap: three.state.devices[2].wrap }] })),
  );

  /* Vault records. */
  const rootVault = A.vault(first, "root", 1);
  valid("a vault written by the root key with the first event", first, { vault: rootVault, revision: 1 });
  valid("a vault written by the first device", first, { vault: A.vault(first, laptop, 2), revision: 2 });
  valid("a vault written by the newer of two devices", approved, { vault: A.vault(approved, phone, 3), revision: 3 });
  valid("a vault written by the root key with a recovery", recovered, { vault: A.vault(recovered, "root", 2), revision: 2 });
  valid("a vault after a revocation", revoked, { vault: A.vault(revoked, laptop, 9), revision: 9 });
  valid(
    "a vault with fields no verifier knows",
    first,
    { vault: A.vault(first, laptop, 2, (v) => ({ ...v, note: "unknown", future: [1, { nested: null }] })), revision: 2 },
  );
  valid(
    "a vault at the largest revision JavaScript counts exactly",
    first,
    { vault: A.vault(first, laptop, 9007199254740991), revision: 9007199254740991 },
  );
  const withVault = (name, rule, c, vault) => invalid(name, rule, c.events, { vault });
  withVault("a vault for another account", "account", first, A.vault(first, laptop, 2, (v) => ({ ...v, account: "usr_other" })));
  withVault("a vault signed against an earlier event", "head", approved, A.vault(first, laptop, 2));
  withVault("a vault signed against no event", "head", first, A.vault(first, laptop, 2, (v) => ({ ...v, head: "" })));
  withVault("a vault signed against another history", "head", first, A.vault(first, laptop, 2, (v) => ({ ...v, head: shared.head })));
  withVault("a vault at another epoch", "epoch", revoked, A.vault(revoked, laptop, 2, (v) => ({ ...v, epoch: 1 })));
  withVault("a vault at revision zero", "revision", first, A.vault(first, laptop, 0));
  withVault("a vault whose signer the state does not hold", "writer", first, A.vault(first, tablet, 2));
  withVault("a vault by the root key after an approval", "writer", approved, A.vault(approved, "root", 2));
  withVault("a vault by the root key after sharing was turned on", "writer", shared, A.vault(shared, "root", 2));
  withVault("a vault by a device that was revoked", "writer", revoked, A.vault(revoked, phone, 2));
  withVault("a vault signed by another device's key", "signature", approved, A.vault(approved, laptop, 2, (v) => v, { key: phone.sign }));
  withVault("a vault signed by the root key in a device's name", "signature", first, A.vault(first, laptop, 2, (v) => v, { key: A.root }));
  withVault("a vault signed as an event", "signature", first, A.vault(first, laptop, 2, (v) => v, { kind: "event" }));
  withVault("a vault whose payload is not JSON", "payload", first, A.vault(first, laptop, 2, () => Buffer.from("not json")));
  withVault("a vault whose payload is a list", "payload", first, A.vault(first, laptop, 2, (v) => Buffer.from(JSON.stringify(Object.values(v)))));
  withVault("a vault with a revision in quotes", "payload", first, A.vault(first, laptop, 2, (v) => ({ ...v, revision: "2" })));
  withVault("a vault with a negative revision", "payload", first, A.vault(first, laptop, 2, (v) => ({ ...v, revision: -1 })));
  withVault("a good vault on a history that is not one", "previous", { events: A.amend(approved, (s) => ({ ...s, prev: "" })) }, A.vault(approved, laptop, 2));
  withVault("an event where the vault belongs", "payload", first, first.events[0]);
  const strict = "Go checks the vault's ciphertext when it opens it. JavaScript and Rust refuse the record.";
  const record = (c, revision) => ({ ...summary(c), revision });
  differs("a vault whose ciphertext is not base64url", goOnly, strict, first.events, { vault: A.vault(first, laptop, 2, (v) => ({ ...v, data: `${v.data}=` })), rule: "data", state: record(first, 2) });
  differs("a vault whose ciphertext is shorter than a nonce and a tag", goOnly, strict, first.events, { vault: A.vault(first, laptop, 2, (v) => ({ ...v, data: b64(material("short vault", 27)) })), rule: "data", state: record(first, 2) });
  differs("a vault with no ciphertext", goOnly, strict, first.events, { vault: A.vault(first, laptop, 2, (v) => ({ ...v, data: "" })), rule: "data", state: record(first, 2) });
  differs(
    "a vault without the field for its ciphertext",
    goOnly,
    "Go reads a field that is left out as empty.",
    first.events,
    { vault: A.vault(first, laptop, 2, ({ data, ...v }) => v), rule: "payload", state: record(first, 2) },
  );
  differs(
    "a vault at a revision JavaScript cannot count exactly",
    goOnly,
    "JavaScript reads 9007199254740993 as 9007199254740992 and refuses a count it cannot hold exactly. Rust refuses what JavaScript would misread.",
    first.events,
    { vault: A.vault(first, laptop, 2, (v) => Buffer.from(JSON.stringify(v).replace('"revision":2', '"revision":9007199254740993'))), rule: "payload", state: record(first, JSON.rawJSON("9007199254740993")) },
  );

  /* Where the three read a payload differently. */
  const jsOnly = { js: "valid", go: "invalid", rust: "invalid" };
  const rustOnly = { js: "valid", go: "valid", rust: "invalid" };
  const spelling = "JavaScript reads the number a JSON token stands for, so 1.0 and 1e0 are 1. Go and Rust read a count only from a whole number as written.";
  differs("a byte-order mark before the payload", jsOnly, "The viewer's text decoder drops a byte-order mark. Go, Rust and the hosted service refuse it.", A.amend(first, (s) => Buffer.concat([Buffer.from([0xef, 0xbb, 0xbf]), Buffer.from(JSON.stringify(s))])), { rule: "payload", state: stateOf(first) });
  differs("an epoch written 1.0", jsOnly, spelling, A.amend(first, (s) => rewritten(s, "epoch", "1.0")), { rule: "payload", state: stateOf(first) });
  differs("an epoch written 1e0", jsOnly, spelling, A.amend(first, (s) => rewritten(s, "epoch", "1e0")), { rule: "payload", state: stateOf(first) });
  differs("a sequence number written 0.0", jsOnly, spelling, A.amend(first, (s) => rewritten(s, "seq", "0.0")), { rule: "payload", state: stateOf(first) });
  differs("a second event numbered 10e-1", jsOnly, spelling, A.amend(approved, (s) => rewritten(s, "seq", "10e-1")), { rule: "payload", state: stateOf(approved) });
  differs(
    "a sequence number written -0",
    rustOnly,
    "JavaScript and Go read -0 as zero. serde_json reads it as a fraction, and a count is not read from one.",
    A.amend(first, (s) => rewritten(s, "seq", "-0")),
    { rule: "payload", state: stateOf(first) },
  );
  differs(
    "an incoming-share key that is false",
    jsOnly,
    "JavaScript takes anything falsy for no key. Go and Rust take a string, null or nothing.",
    A.amend(first, (s) => rewritten(s, "inbox", "false")),
    { rule: "payload", state: stateOf(first) },
  );
  const folded = "Go matches a field's name without regard to case, the long s and the Kelvin sign included. JavaScript and Rust take the exact name, and Rust refuses a payload Go would read differently.";
  differs("a field's name in capitals", goOnly, folded, A.amend(first, (s) => renamed(s, "account", '"ACCOUNT"')), { rule: "payload", state: stateOf(first) });
  differs("a field's name capitalized", goOnly, folded, A.amend(first, (s) => renamed(s, "seq", '"Seq"')), { rule: "payload", state: stateOf(first) });
  differs("a field's name with a long s", goOnly, folded, A.amend(first, (s) => renamed(s, "seq", `"${String.fromCodePoint(0x17f)}eq"`)), { rule: "payload", state: stateOf(first) });
  differs("a field's name with a Kelvin sign", goOnly, folded, A.amend(first, (s) => renamed(s, "kind", `"${String.fromCodePoint(0x212a)}ind"`)), { rule: "payload", state: stateOf(first) });
  differs("a device field's name in capitals", goOnly, folded, A.amend(first, (s) => Buffer.from(JSON.stringify(s).replace('"wrap":', '"WRAP":'))), { rule: "payload", state: stateOf(first) });
  // The one place two implementations accept the same bytes and read two
  // different states from them.
  const twoNames = A.amend(first, (s) => Buffer.from(JSON.stringify(s).replace('"name":"Laptop"', '"name":"Laptop","Name":"Other"')));
  differs(
    "a device name given twice, the second time capitalized",
    { js: "valid", go: "valid", rust: "invalid" },
    "JavaScript reads the device as Laptop and Go as Other: the same signed bytes, two states. Rust refuses them.",
    twoNames,
    { rule: "payload", state: { js: stateOf(first), go: { ...stateOf(first), devices: [{ id: laptop.id, name: "Other" }] } } },
  );
  // A field given twice. Both readers take the last, unless it is null.
  const twice = "JavaScript and Go take the last of two. Rust refuses a payload that says a thing twice, because the two do not always agree on which one counts.";
  differs(
    "a field written twice, where the last one counts",
    rustOnly,
    twice,
    A.amend(first, (s) => Buffer.from(`{"kind":"approve","epoch":7,${JSON.stringify(s).slice(1)}`)),
    { rule: "payload", state: stateOf(first) },
  );
  differs(
    "a field written twice, the same both times",
    rustOnly,
    twice,
    A.amend(first, (s) => Buffer.from(`{"epoch":1,${JSON.stringify(s).slice(1)}`)),
    { rule: "payload", state: stateOf(first) },
  );
  differs(
    "a field written twice inside an unknown field",
    rustOnly,
    twice,
    A.amend(first, (s) => Buffer.from(JSON.stringify({ ...s, future: 0 }).replace('"future":0', '"future":{"a":1,"a":2}'))),
    { rule: "payload", state: stateOf(first) },
  );
  const nullLast = "Go leaves a field as it was when it meets null, so it keeps the value before; JavaScript takes the null. The same signed bytes, two states. Rust refuses them.";
  differs(
    "sharing turned on, with null after the key",
    goOnly,
    nullLast,
    A.amend(shared, (s) => Buffer.from(JSON.stringify(s).replace(/\}$/, ',"inbox":null}'))),
    { rule: "payload", state: stateOf(shared) },
  );
  differs(
    "a first event with an incoming-share key and null after it",
    rustOnly,
    nullLast,
    A.amend(first, (s) => Buffer.from(`{"inbox":${JSON.stringify(A.inbox("a"))},${JSON.stringify(s).slice(1, -1)},"inbox":null}`)),
    { rule: "payload", state: { js: stateOf(first), go: { ...stateOf(first), inbox: A.inbox("a") } } },
  );
  const zeroed = "Go reads a field that is null or left out as empty or zero. JavaScript and Rust need the field.";
  differs("null where the sequence number belongs", goOnly, zeroed, A.amend(first, (s) => rewritten(s, "seq", "null")), { rule: "payload", state: stateOf(first) });
  differs("a first event without a sequence number", goOnly, zeroed, A.amend(first, ({ seq, ...s }) => s), { rule: "payload", state: stateOf(first) });
  differs("a first event without the field for its predecessor", goOnly, zeroed, A.amend(first, ({ prev, ...s }) => s), { rule: "payload", state: stateOf(first) });
  differs("null where the first event's predecessor belongs", goOnly, zeroed, A.amend(first, (s) => rewritten(s, "prev", "null")), { rule: "payload", state: stateOf(first) });
  const nameless = named("nameless", "");
  differs("a device whose name is null", goOnly, zeroed, A.amend(nameless, (s) => Buffer.from(JSON.stringify(s).replace('"name":""', '"name":null'))), { rule: "payload", state: stateOf(nameless) });
  differs("a device without the field for its name", goOnly, zeroed, A.amend(nameless, (s) => Buffer.from(JSON.stringify(s).replace('"name":"",', ""))), { rule: "payload", state: stateOf(nameless) });
  const lost = named("name lost", String.fromCodePoint(0xfffd));
  differs(
    "a byte that is not UTF-8 in a device name",
    goOnly,
    "Go reads the byte as U+FFFD. The viewer and Rust refuse text that is not UTF-8.",
    A.amend(lost, (s) => {
      const text = Buffer.from(JSON.stringify(s));
      const at = text.indexOf(Buffer.from([0xef, 0xbf, 0xbd]));
      return Buffer.concat([text.subarray(0, at), Buffer.from([0xff]), text.subarray(at + 3)]);
    }),
    { rule: "payload", state: stateOf(lost) },
  );
  differs(
    "half of a surrogate pair escaped in a device name",
    goOnly,
    "A name holds whole characters. Go reads the half as U+FFFD and would write the device back under that name; the viewer and the service refuse the name, and serde_json refuses the escape.",
    A.amend(lost, (s) => Buffer.from(JSON.stringify(s).replace(String.fromCodePoint(0xfffd), `${backslash}ud83d`))),
    { rule: "payload", state: stateOf(lost) },
  );
  // In an unknown field, because a case file that held the half itself could
  // not be read by serde_json either.
  differs(
    "half of a surrogate pair escaped in an unknown field",
    rustOnly,
    "JavaScript keeps the half and Go reads it as U+FFFD. serde_json refuses the escape, and with it the payload.",
    A.amend(first, (s) => Buffer.from(JSON.stringify({ ...s, note: "half" }).replace('"note":"half"', `"note":"${backslash}ud83d"`))),
    { rule: "payload", state: stateOf(first) },
  );
  differs(
    "an unknown field nested 200 deep",
    rustOnly,
    "serde_json stops at 128 levels, as it does for a session document.",
    A.amend(first, (s) => Buffer.from(JSON.stringify({ ...s, deep: 0 }).replace('"deep":0', `"deep":${"[".repeat(200)}${"]".repeat(200)}`))),
    { rule: "payload", state: stateOf(first) },
  );
  const bulky = A.amend(first, (s) => ({ ...s, padding: "p".repeat(24 * 1024) }));
  if (bulky[0].payload.length <= 32 * 1024) throw new Error("the oversized event is no longer oversized");
  differs(
    "an event longer than the hosted service stores",
    rustOnly,
    "The service refuses an event of more than 32 KiB as text, and Rust keeps to that limit before decoding anything. Go and the viewer have no limit of their own.",
    bulky,
    { rule: "signed", state: stateOf(first) },
  );

  /* Signing keys of small order: refused where they would sign. */
  const identity = b64(Buffer.concat([Buffer.from([1]), Buffer.alloc(31)]));
  const anyone = b64(Buffer.concat([Buffer.from([1]), Buffer.alloc(63)]));
  invalid("a first event under the identity point as root key, signed by no one", "key", [{ payload: b64(JSON.stringify({ ...first.state, root: identity })), signature: anyone }]);
  // Enrolling such a device is not refused: nothing is ever checked under its
  // key, so it can do nothing, and neither can anyone in its name.
  const weakBox = keyPair("x", "usr_fixture/weak/box").public;
  const weakDevice = { id: digest(`${identity}.${weakBox}`), name: "Weak", sign: identity, box: weakBox, wrap: wrap(weakBox, A.vaultKey(1), A.context(1), "usr_fixture/weak/1") };
  const enrolled = A.extend(first, { kind: "approve", devices: [...first.state.devices, weakDevice] }, laptop);
  valid("a device whose signing key is the identity point is enrolled", enrolled);
  const inItsName = { ...enrolled.state, seq: 2, prev: enrolled.head, signer: weakDevice.id, devices: [...enrolled.state.devices, A.entry(phone, 1)] };
  invalid("an approval in the name of a device whose key is the identity point", "key", [...enrolled.events, { payload: b64(JSON.stringify(inItsName)), signature: anyone }]);
  invalid(
    "a vault in the name of a device whose key is the identity point",
    "key",
    enrolled.events,
    { vault: { payload: A.vault(enrolled, laptop, 2, (v) => ({ ...v, signer: weakDevice.id })).payload, signature: anyone } },
  );

  return { signed: out.signed, cases: out.cases };
}

/* ------------------------------------------------------------------ keys */

const backslashed = (c) => `${String.fromCharCode(0x5c)}u${c.charCodeAt(0).toString(16).padStart(4, "0")}`;
/**
 * JSON as the Go CLI writes it: what JSON.stringify writes, with the five
 * characters Go also escapes, for the sake of HTML.
 */
const HTML_ESCAPED = new RegExp(`[<>&${String.fromCodePoint(0x2028)}${String.fromCodePoint(0x2029)}]`, "g");
export const goJSON = (value) => JSON.stringify(value).replace(HTML_ESCAPED, backslashed);
/** A state as the Go CLI writes it: the incoming-share key first when there is one. */
export const writtenState = (s) =>
  goJSON({
    ...(s.inbox ? { inbox: s.inbox } : {}),
    account: s.account,
    seq: s.seq,
    prev: s.prev,
    kind: s.kind,
    signer: s.signer,
    root: s.root,
    recovery: s.recovery,
    recovery_box: s.recovery_box,
    recovery_wrap: s.recovery_wrap,
    epoch: s.epoch,
    devices: s.devices.map((d) => ({ id: d.id, name: d.name, sign: d.sign, box: d.box, wrap: d.wrap })),
  });
export const writtenVault = (v) => goJSON({ account: v.account, revision: v.revision, head: v.head, epoch: v.epoch, signer: v.signer, data: v.data });
export const writtenRequest = (r) => goJSON({ account: r.account, root: r.root, name: r.name, sign: r.sign, box: r.box, nonce: r.nonce, created: r.created });
/** Whether a statement's payload is, byte for byte, what a client writes for the value it holds. */
const asWritten = (signed, write) => {
  const raw = unb64(signed.payload);
  try {
    return raw.equals(Buffer.from(write(JSON.parse(raw.toString()))));
  } catch {
    return false;
  }
};

const BASE32 = "ABCDEFGHIJKLMNOPQRSTUVWXYZ234567";
/** The code a device request is confirmed with: sixty bits of its hash. */
export function confirmation(signed) {
  const bits = Array.from(createHash("sha256").update(unb64(signed.payload)).digest().subarray(0, 8), (byte) => byte.toString(2).padStart(8, "0")).join("");
  const code = Array.from({ length: 12 }, (_, i) => BASE32[parseInt(bits.slice(i * 5, i * 5 + 5), 2)]).join("");
  return `${code.slice(0, 4)}-${code.slice(4, 8)}-${code.slice(8)}`;
}

const le32 = (hex) => Buffer.from(hex, "hex");
const FIELD_SIZE = "edffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff7f";
/**
 * The X25519 points of small order, by the seven numbers that spell them.
 * Whatever private key meets one, the two agree on 32 zero bytes, so a
 * wrapper under one can be made by anyone and opened by anyone.
 */
const SMALL_POINTS = [
  ["zero", "00".repeat(32)],
  ["one", `01${"00".repeat(31)}`],
  ["a point of order eight", "e0eb7a7c3b41b8ae1656e3faf19fc46ada098deb9c32b1fd866205165f49b800"],
  ["the other point of order eight", "5f9c95bca3508c24b1d0b1559c83ef5b04445cc4581c8e86d8224eddd09f1157"],
  ["one less than the field's size", `ec${"ff".repeat(30)}7f`],
  ["the field's size", FIELD_SIZE],
  ["one more than the field's size", `ee${"ff".repeat(30)}7f`],
].map(([name, hex]) => [name, le32(hex)]);

/** A wrapper under a one-time public key given as bytes, sealed under what `secret` derives. */
function wrapperUnder(ephemeralPublic, secret, recipient, key, context, label) {
  const info = `slink/wrap/v1/${context}/${b64(ephemeralPublic)}/${recipient}`;
  const derived = Buffer.from(hkdfSync("sha256", secret, Buffer.alloc(0), info, 32));
  return `${b64(ephemeralPublic)}.${seal(derived, key, info, label)}`;
}

function keyCases() {
  const named = (list) => (item) => {
    if (list.some((other) => other.name === item.name)) throw new Error(`two cases are named ${item.name}`);
    list.push(item);
  };
  const std = (bytes) => Buffer.from(bytes).toString("base64");
  const flipped = (text, at) => {
    const raw = unb64(text);
    raw[at < 0 ? raw.length + at : at] ^= 1;
    return b64(raw);
  };

  /* Boxes: bytes under a key, bound to a context. */
  const boxes = [];
  {
    const add = named(boxes);
    const key = material("box/key");
    const context = "slink/vault/v1/usr_fixture/root/1";
    const opens = (name, plain, context, label) => add({ name, key: b64(key), context, box: seal(key, plain, context, label), expect: "opens", plaintext: std(plain) });
    const refused = (name, box, change = {}) => add({ name, key: b64(key), context, box, expect: "refused", ...change });
    opens("a box", EMPTY_VAULT, context, "box/plain");
    opens("a box of nothing", Buffer.alloc(0), context, "box/nothing");
    opens("a box of one byte", Buffer.from([0]), context, "box/one byte");
    opens("a box of 64 bytes that are not text", material("box/bytes", 64), context, "box/bytes");
    opens("a box with no context", EMPTY_VAULT, "", "box/no context");
    opens("a box whose context is not ASCII", EMPTY_VAULT, `usr_${String.fromCodePoint(0xe9)}/${String.fromCodePoint(0x1f4bb)}`, "box/unicode");
    const good = boxes[0].box;
    const size = unb64(good).length;
    for (const [where, at] of [["the nonce's first byte", 0], ["the nonce's last byte", 11], ["the first byte after the nonce", 12], ["the last byte before the tag", -17], ["the tag's first byte", -16], ["the tag's last byte", -1]]) {
      refused(`one bit changed in ${where}`, flipped(good, at));
    }
    refused("another key", good, { key: b64(material("box/other key")) });
    refused("a context one character longer", good, { context: `${context}0` });
    refused("a context one character shorter", good, { context: context.slice(0, -1) });
    refused("a context in capitals", good, { context: context.toUpperCase() });
    refused("no context where there was one", good, { context: "" });
    refused("a context where there was none", boxes[4].box);
    refused("the last byte missing", b64(unb64(good).subarray(0, size - 1)));
    refused("one byte added", b64(Buffer.concat([unb64(good), Buffer.from([0])])));
    refused("a nonce and a tag with nothing between, from a box that held something", b64(Buffer.concat([unb64(good).subarray(0, 12), unb64(good).subarray(-16)])));
    refused("one byte short of a nonce and a tag", b64(unb64(boxes[1].box).subarray(0, 27)));
    refused("a nonce alone", b64(unb64(good).subarray(0, 12)));
    refused("nothing", "");
    if (!/[-_]/.test(good) || good.length % 4 === 0) throw new Error("the box lost a character the two alphabets differ on, or its odd length");
    refused("a box with padding", `${good}=`);
    refused("a box in the other base64 alphabet", otherAlphabet(good));
    refused("a box with stray bits in its last character", strayBits(good));
    refused("a box that ends with a line break", `${good}\n`);
    refused("a box with a space in it", `${good.slice(0, 8)} ${good.slice(8)}`);
  }

  /* Wrappers: a key in a box only one X25519 key opens. */
  const wrappers = [];
  const recipients = [];
  const A = account("usr_fixture");
  {
    const add = named(wrappers);
    const recipient = keyPair("x", "wrapper/recipient");
    const other = keyPair("x", "wrapper/other");
    const key = material("wrapper/key");
    const context = A.context(1);
    const base = { private: b64(recipient.seed), public: recipient.public, context };
    /** One made the way a client makes it, with the one-time key it used. */
    const made = (name, label, to = recipient, context = base.context, wrapped = key) =>
      add({ name, private: b64(to.seed), public: to.public, context, wrapper: wrap(to.public, wrapped, context, label), expect: "opens", key: b64(wrapped), ephemeral: b64(keyPair("x", `ephemeral/${label}`).seed) });
    made("a wrapper", "wrapper/plain");
    made("a wrapper to another key", "wrapper/other", other);
    made("a wrapper at the next epoch", "wrapper/epoch", recipient, A.context(2));
    made("a wrapper for a share's key", "wrapper/named", recipient, `named/${"ab".repeat(32)}/usr_fixture/${A.root.public}/3`);
    made("a wrapper with no context", "wrapper/no context", recipient, "");
    made("a wrapper of a key of zeros", "wrapper/zeros", recipient, context, Buffer.alloc(32));
    const good = wrappers[0].wrapper;
    const [half, box] = good.split(".");
    const refused = (name, wrapper, change = {}) => add({ name, ...base, wrapper, expect: "refused", ...change });

    // The same point spelt two other ways. Every library reads both as the
    // point, so the wrapper opens; the spelling is part of what the key is
    // derived from, so it is not the same wrapper.
    const ephemeral = keyPair("x", "ephemeral/wrapper/spelling");
    const secret = diffieHellman({ privateKey: ephemeral.private, publicKey: publicKey("x", unb64(recipient.public)) });
    const topBit = unb64(ephemeral.public);
    topBit[31] |= 0x80;
    add({ name: "a one-time key with its unused top bit set", ...base, wrapper: wrapperUnder(topBit, secret, recipient.public, key, context, "wrapper/top bit"), expect: "opens", key: b64(key) });
    // Nine is the base point, and so is the field's size and nine: what a
    // private key agrees on with it is its own public key.
    const basePoint = le32(`f6${"ff".repeat(30)}7f`);
    add({ name: "the base point spelt past the field's size as the one-time key", ...base, wrapper: wrapperUnder(basePoint, unb64(recipient.public), recipient.public, key, context, "wrapper/base point"), expect: "opens", key: b64(key) });

    refused("a wrapper opened with another key", good, { private: b64(other.seed), public: other.public });
    refused("a wrapper opened with the one-time key it was made with", good, { private: wrappers[0].ephemeral, public: half });
    refused("a wrapper opened at the next epoch", good, { context: A.context(2) });
    refused("a wrapper opened for another account", good, { context: account("usr_other").context(1) });
    refused("a wrapper opened with no context", good, { context: "" });
    refused("one bit changed in the one-time key", `${flipped(half, 0)}.${box}`);
    refused("one bit changed in the nonce", `${half}.${flipped(box, 0)}`);
    refused("one bit changed in the wrapped key", `${half}.${flipped(box, 12)}`);
    refused("one bit changed in the tag", `${half}.${flipped(box, -1)}`);
    refused("another wrapper's one-time key", `${wrappers[1].wrapper.split(".")[0]}.${box}`);
    refused("the two halves the other way round", `${box}.${half}`);
    refused("a wrapper one character short", good.slice(0, -1));
    refused("a wrapper one character long", `${good}A`);
    refused("a wrapper with its dot one place late", `${good.slice(0, 43)}A.${good.slice(45)}`);
    refused("a wrapper with no dot", good.replace(".", "A"));
    refused("a wrapper with padding", `${good}=`);
    refused("a wrapper in the other base64 alphabet", `${"+".repeat(43)}.${"/".repeat(80)}`);
    refused("no wrapper", "");
    const info = `slink/wrap/v1/${context}/${half}/${recipient.public}`;
    const derived = Buffer.from(hkdfSync("sha256", secret0(recipient, half), Buffer.alloc(0), info, 32));
    refused("a wrapper around 31 bytes", `${half}.${seal(derived, key.subarray(0, 31), info, "wrapper/31")}`);
    refused("a wrapper around 33 bytes", `${half}.${seal(derived, Buffer.concat([key, Buffer.from([0])]), info, "wrapper/33")}`);

    // A point of small order as the one-time key, with the box sealed under
    // what it agrees on with every key: 32 zero bytes. A reader that did not
    // refuse the point would open it.
    for (const [what, point] of SMALL_POINTS) {
      const zero = agreed(recipient, point);
      if (zero && !zero.equals(Buffer.alloc(32))) throw new Error(`${what} is not a point of small order`);
      refused(`${what} as the one-time key, sealed under what it agrees on`, wrapperUnder(point, Buffer.alloc(32), recipient.public, key, context, `wrapper/small/${what}`));
    }
    const signed = Buffer.from(SMALL_POINTS[1][1]);
    signed[31] |= 0x80;
    refused("one with its top bit set as the one-time key, sealed under what it agrees on", wrapperUnder(signed, Buffer.alloc(32), recipient.public, key, context, "wrapper/small/top bit"));

    /* Keys nothing is wrapped to. */
    const never = named(recipients);
    for (const [what, point] of SMALL_POINTS) never({ name: `${what} as the key to wrap to`, public: b64(point), expect: "refused" });
    never({ name: "a key of 31 bytes to wrap to", public: b64(unb64(recipient.public).subarray(0, 31)), expect: "refused" });
    never({ name: "a key with padding to wrap to", public: `${recipient.public}=`, expect: "refused" });
    never({ name: "no key to wrap to", public: "", expect: "refused" });
    never({ name: "a key to wrap to", public: recipient.public, expect: "wraps" });
  }

  /* Signatures: a seed and a payload have one. */
  const signatures = [];
  {
    const add = named(signatures);
    const key = keyPair("ed", "signature/key");
    for (const kind of KINDS) {
      const signed = sign(kind, { statement: "fixture", kind }, key);
      add({ name: `a statement signed as ${kind}`, kind, seed: b64(key.seed), public: key.public, ...signed });
    }
    const other = keyPair("ed", "signature/other key");
    add({ name: "a statement signed by another key", kind: "event", seed: b64(other.seed), public: other.public, ...sign("event", { statement: "fixture", kind: "event" }, other) });
    add({ name: "an empty object, signed", kind: "event", seed: b64(key.seed), public: key.public, ...sign("event", {}, key) });
  }

  /* Device requests. */
  const requests = [];
  {
    const add = named(requests);
    const device = (label) => ({ sign: keyPair("ed", `request/${label}/sign`), box: keyPair("x", `request/${label}/box`) });
    const phone = device("phone");
    const value = (d, name = "Phone") => ({ account: "usr_fixture", root: A.root.public, name, sign: d.sign.public, box: d.box.public, nonce: b64(material("request/nonce", 24)), created: 1760000000000 });
    const good = value(phone);
    /** One a client writes: its id, its code, and the seed that signs it again. */
    const verified = (name, request, d = phone) => {
      const signed = signBytes("request", Buffer.from(writtenRequest(request)), d.sign);
      add({ name, ...signed, expect: "verified", id: digest(`${request.sign}.${request.box}`), code: confirmation(signed), seed: b64(d.sign.seed) });
      return signed;
    };
    const first = verified("a device request", good);
    verified("a request from a device with no name", value(phone, ""));
    verified("a request with a name of 80 bytes", value(phone, String.fromCodePoint(0x1f4bb).repeat(20)));
    verified("a request whose name holds what Go escapes", value(phone, `<Phone & "tablet">${String.fromCodePoint(0x2028)}`));
    verified("a request dated at the first millisecond", { ...good, created: 0 });
    verified("a request from another device", value(device("tablet"), "Tablet"), device("tablet"));
    // Not what a client writes, and read the same by all three.
    const other = (name, bytes, expect, why, key = phone.sign, kind = "request") => {
      const signed = signBytes(kind, Buffer.from(bytes), key);
      add({ name, ...signed, expect, ...(expect === "verified" ? { id: digest(`${good.sign}.${good.box}`), code: confirmation(signed) } : {}), ...(why ? { why } : {}) });
    };
    const text = JSON.stringify(good);
    other("a request with a field no reader knows", JSON.stringify({ ...good, platform: "unknown" }), "verified");
    other("a request with its fields in another order", JSON.stringify(Object.fromEntries(Object.entries(good).reverse())), "verified");
    other("a request with white space around it", `\n ${JSON.stringify(good, null, 1)}\n`, "verified");
    other("a request signed by another device's key", text, "refused", null, device("tablet").sign);
    other("a request signed as an event", text, "refused", null, phone.sign, "event");
    add({ name: "a request changed after it was signed", payload: b64(text.replace('"Phone"', '"Phonf"')), signature: first.signature, expect: "refused" });
    add({ name: "a request with a signature one byte short", payload: first.payload, signature: b64(unb64(first.signature).subarray(0, 63)), expect: "refused" });
    add({ name: "a request whose payload has padding", payload: `${first.payload}=`, signature: first.signature, expect: "refused" });
    other("a request that is not JSON", "not json", "refused");
    other("a request that is a list", JSON.stringify(Object.values(good)), "refused");
    other("a request with no signing key", JSON.stringify({ ...good, sign: undefined }), "refused");
    other("a request with a signing key of 31 bytes", JSON.stringify({ ...good, sign: b64(unb64(good.sign).subarray(0, 31)) }), "refused");
    // The identity point as the device's key: the signature that fits every
    // payload under it. A device that could be enrolled this way would be
    // anyone's.
    const identity = b64(Buffer.concat([Buffer.from([1]), Buffer.alloc(31)]));
    add({ name: "a request in the name of the identity point, signed by no one", payload: b64(JSON.stringify({ ...good, sign: identity })), signature: b64(Buffer.concat([Buffer.from([1]), Buffer.alloc(63)])), expect: "refused" });

    // What the hosted service refuses when a request is uploaded. Go and the
    // viewer leave it to the service and take the request as signed.
    const service = "The hosted service refuses the request when it is uploaded. Go and the viewer leave that to the service; Rust keeps to the service's rule.";
    const lenient = { js: "verified", go: "verified", rust: "refused" };
    other("a request with a name of 81 bytes", JSON.stringify(value(phone, "n".repeat(81))), lenient, service);
    other("a request with a nonce of 23 bytes", JSON.stringify({ ...good, nonce: b64(material("request/nonce", 23)) }), lenient, service);
    other("a request with an agreement key of 31 bytes", JSON.stringify({ ...good, box: b64(unb64(good.box).subarray(0, 31)) }), lenient, service);
    other("a request with a root key that is a word", JSON.stringify({ ...good, root: "root" }), lenient, service);
    other("a request with a fraction for its time", text.replace('"created":1760000000000', '"created":1760000000000.5'), { js: "verified", go: "refused", rust: "refused" }, "JavaScript reads any number. The service refuses a time that is not a whole number, as Go and Rust do.");
    other("a request with its time in quotes", text.replace('"created":1760000000000', '"created":"1760000000000"'), { js: "verified", go: "refused", rust: "refused" }, "JavaScript compares the text as a number. The service refuses it, as Go and Rust do.");
    other("a request without a name", JSON.stringify({ ...good, name: undefined }), lenient, "Go reads a field that is left out as empty and the viewer does not look. The service refuses a name that is not text.");
    other("a request without a time", JSON.stringify({ ...good, created: undefined }), { js: "verified", go: "verified", rust: "refused" }, "Go reads a field that is left out as zero and the viewer compares nothing with a number. The service refuses it.");
    const folded = "Go matches a field's name without regard to case and JavaScript takes the exact name. Rust refuses a payload the two would read differently.";
    other("a request with its signing key under a name in capitals", text.replace('"sign":', '"SIGN":'), { js: "refused", go: "verified", rust: "refused" }, folded);
    other("a request with a second name, capitalized", text.replace('"name":"Phone"', '"name":"Phone","Name":"Other"'), { js: "verified", go: "verified", rust: "refused" }, "JavaScript reads the device as Phone and Go as Other: the same signed bytes, two names. Rust refuses them.");
    other("a request with a field given twice", `{"created":1,${text.slice(1)}`, { js: "verified", go: "verified", rust: "refused" }, "JavaScript and Go take the last of two. Rust refuses a payload that says a thing twice.");
    const bulky = JSON.stringify({ ...good, padding: "p".repeat(3200) });
    if (b64(bulky).length <= 4096) throw new Error("the oversized request is no longer oversized");
    other("a request longer than the hosted service stores", bulky, lenient, "The service refuses a request of more than 4 KiB as text. Go and the viewer have no limit of their own.");
  }

  /* A device unlocks an account: its wrapper, then the vault. */
  const devices = [];
  const recoveries = [];
  {
    const add = named(devices);
    const laptop = A.device("laptop", "Laptop");
    const phone = A.device("phone", "Phone");
    const first = A.init(laptop);
    const approved = A.approve(first, laptop, phone);
    const revoked = A.revoke(approved, laptop, phone);
    const shared = A.sharing(first, laptop, "a");
    const recovered = A.recover(first, phone);
    const keyring = Buffer.from(JSON.stringify({ inboxes: { [A.inbox("a")]: b64(keyPair("x", "usr_fixture/inbox/a").seed) }, version: 2, shares: [] }));
    const record = (c, by, revision, options) => ({ account: A.name, events: c.events, vault: A.vault(c, by, revision, (value) => value, options) });
    const unlock = (name, c, by, revision, device, expect, { contents = EMPTY_VAULT, record: given, private: own = device.box.seed } = {}) =>
      add({
        name,
        ...(given ?? record(c, by, revision, { contents })),
        device: device.id,
        private: b64(own),
        expect,
        ...(expect === "opens" ? { vault_key: b64(A.vaultKey(c.state.epoch)), contents: std(contents) } : {}),
      });
    unlock("the first device opens the vault the root key wrote", first, "root", 1, laptop, "opens");
    unlock("the first device opens the vault it wrote", first, laptop, 2, laptop, "opens");
    unlock("the second device opens the vault the first wrote", approved, laptop, 3, phone, "opens");
    unlock("the first device opens the vault the second wrote", approved, phone, 4, laptop, "opens");
    unlock("the device that stays opens the vault after a revocation", revoked, laptop, 5, laptop, "opens");
    unlock("a device the recovery key enrolled opens the vault", recovered, "root", 2, phone, "opens");
    unlock("a vault that holds an incoming-share key", shared, laptop, 2, laptop, "opens", { contents: keyring });
    unlock("a vault of nothing", first, laptop, 6, laptop, "opens", { contents: Buffer.alloc(0) });
    unlock("the revoked device after its revocation", revoked, laptop, 5, phone, "refused");
    unlock("a device the history never held", first, laptop, 2, phone, "refused");
    unlock("the first device's entry with another device's key", approved, laptop, 3, laptop, "refused", { private: phone.box.seed });
    unlock("the first device's entry with the recovery key", first, laptop, 2, laptop, "refused", { private: A.recovery.seed });
    // The record verifies: a vault's ciphertext is not opened to verify it.
    const sealedFor = (context, label) => (value) => ({ ...value, data: seal(A.vaultKey(1), EMPTY_VAULT, context, label) });
    const moved = (name, change) => unlock(name, first, laptop, 2, laptop, "refused", { record: { account: A.name, events: first.events, vault: A.vault(first, laptop, 2, change) } });
    moved("a vault sealed for the next epoch", sealedFor(`slink/vault/v1/${A.context(2)}`, "device/epoch"));
    moved("a vault sealed for another account", sealedFor(`slink/vault/v1/${account("usr_other").context(1)}`, "device/account"));
    moved("a vault sealed as a recovery package", sealedFor(`slink/recovery/v1/${A.name}/${A.root.public}`, "device/purpose"));
    moved("a vault sealed under the next epoch's key", (value) => ({ ...value, data: seal(A.vaultKey(2), EMPTY_VAULT, `slink/vault/v1/${A.context(1)}`, "device/key") }));
    moved("a vault with one bit of its ciphertext changed", (value) => ({ ...value, data: flipped(value.data, 20) }));

    /* A recovery key unlocks an account: the package, the wrapper, the vault. */
    const recover = named(recoveries);
    const opened = { root: b64(A.root.seed), agreement: b64(A.recovery.seed) };
    const recovers = (name, c, by, revision, token = A.recoveryKey) =>
      recover({ name, ...record(c, by, revision), token, expect: "opens", ...opened, vault_key: b64(A.vaultKey(c.state.epoch)), contents: std(EMPTY_VAULT) });
    const lost = (name, token, given = record(first, laptop, 2)) => recover({ name, ...given, token, expect: "refused" });
    recovers("the recovery key opens a new account", first, "root", 1);
    recovers("the recovery key opens an account with two devices", approved, phone, 4);
    recovers("the recovery key opens an account after a revocation", revoked, laptop, 5);
    recovers("the recovery key opens an account it has recovered before", recovered, "root", 2);
    recovers("a recovery key with white space around it", first, laptop, 2, `  ${A.recoveryKey}\n`);
    const [version, fingerprint, secret] = A.recoveryKey.split(".");
    const stranger = account("usr_other");
    lost("another account's recovery key", stranger.recoveryKey);
    lost("a recovery key with another account's fingerprint", `${version}.${stranger.recoveryKey.split(".")[1]}.${secret}`);
    lost("a recovery key with another secret", `${version}.${fingerprint}.${b64(material("recovery/other secret"))}`);
    lost("a recovery key of another version", `slr2.${fingerprint}.${secret}`);
    lost("a recovery key in capitals", A.recoveryKey.toUpperCase());
    lost("a recovery key without its version", `${fingerprint}.${secret}`);
    lost("a recovery key with a fourth part", `${A.recoveryKey}.${secret}`);
    lost("a recovery key whose secret has padding", `${A.recoveryKey}=`);
    lost("a recovery key whose secret is one byte short", `${version}.${fingerprint}.${b64(unb64(secret).subarray(0, 31))}`);
    lost("a recovery key with a space in it", `${version}.${fingerprint}. ${secret}`);
    lost("no recovery key", "");
    // A history whose package or wrapper is not what the key should find.
    // Each is signed by the root key and verifies: neither is opened to
    // verify a history.
    const context = `slink/recovery/v1/${A.name}/${A.root.public}`;
    const rebuilt = (change) => {
      const events = A.amend(first, change);
      const c = { events, state: change(structuredClone(first.state)), head: hash(events.at(-1)) };
      return { account: A.name, events, vault: A.vault(c, laptop, 2) };
    };
    const packaged = (plain, context, label) => (state) => ({ ...state, recovery_box: seal(A.recoverySecret, plain, context, label) });
    const both = Buffer.concat([A.root.seed, A.recovery.seed]);
    lost("a package sealed for another account", A.recoveryKey, rebuilt(packaged(both, `slink/recovery/v1/usr_other/${A.root.public}`, "recovery/account")));
    lost("a package sealed as a vault", A.recoveryKey, rebuilt(packaged(both, `slink/vault/v1/${A.context(1)}`, "recovery/purpose")));
    lost("a package that holds another root key", A.recoveryKey, rebuilt(packaged(Buffer.concat([stranger.root.seed, A.recovery.seed]), context, "recovery/root")));
    lost("a package that holds another agreement key", A.recoveryKey, rebuilt(packaged(Buffer.concat([A.root.seed, stranger.recovery.seed]), context, "recovery/agreement")));
    lost("a package with its two keys the other way round", A.recoveryKey, rebuilt(packaged(Buffer.concat([A.recovery.seed, A.root.seed]), context, "recovery/order")));
    lost("a recovery wrapper made for the next epoch", A.recoveryKey, rebuilt((state) => ({ ...state, recovery_wrap: A.recoveryWrap(2) })));
    lost("a recovery wrapper made for the device", A.recoveryKey, rebuilt((state) => ({ ...state, recovery_wrap: state.devices[0].wrap })));
  }
  return { boxes, wrappers, recipients, signatures, requests, devices, recoveries, shares: shareCases() };
}

export const writtenInvite = (v) =>
  goJSON({ account: v.account, root: v.root, head: v.head, signer: v.signer, sha256: v.sha256, inbox: v.inbox, epoch: v.epoch, wrap: v.wrap, invitation: { id: v.invitation.id, email: v.invitation.email, commitment: v.invitation.commitment } });
export const writtenGrant = (v) =>
  goJSON({ account: v.account, root: v.root, head: v.head, signer: v.signer, share_id: v.share_id, invite_id: v.invite_id, sha256: v.sha256, recipient: v.recipient, recipient_root: v.recipient_root, inbox: v.inbox, epoch: v.epoch, wrap: v.wrap, claim_hash: v.claim_hash });
/** The proof a recipient sends with a claim: that its signer held the invitation's secret. */
export const bindingProof = (secret, claim) => b64(createHmac("sha256", secret).update("slink/recipient-binding/v1\0").update(unb64(claim.payload)).digest());

/**
 * Shares for named people: the sender's invitation, the recipient's claim
 * with its proof, and the grant that answers it. A case is what a sender
 * holds when it decides whether to release the share's key.
 */
function shareCases() {
  const shares = [];
  const push = (item) => {
    if (shares.some((other) => other.name === item.name)) throw new Error(`two cases are named ${item.name}`);
    shares.push(item);
  };
  const S = account("usr_fixture");
  const laptop = S.device("laptop", "Laptop");
  const desktop = S.device("desktop", "Desktop");
  const sender = S.sharing(S.init(laptop), laptop, "a");
  const R = account("usr_recipient");
  const phone = R.device("phone", "Phone");
  const tablet = R.device("tablet", "Tablet");
  const alone = R.sharing(R.init(phone), phone, "a");
  const two = R.approve(alone, phone, tablet);

  const sealed = envelope(Buffer.from('{"schema":"session/v0"}'), "share");
  const share = { id: "sharefxtvr2345", sha256: createHash("sha256").update(sealed.data).digest("hex") };
  const key = unb64(sealed.key);
  const secret = material("share/invitation secret");
  const invitation = { id: b64(material("share/invitation id", 24)), email: "recipient@example.com", commitment: digest(secret) };
  const context = (c) => `named/${share.sha256}/${c.state.account}/${c.state.root}/${c.state.epoch}`;
  const inboxes = (who, ...labels) => Object.fromEntries(labels.map((label) => [who.inbox(label), b64(who.inboxKey(label).seed)]));

  /** The sender's statement for the invitation, signed when its history stood at `c`. */
  const invite = (c, label = "share/invite") => ({
    statement: signBytes("named-invite", Buffer.from(writtenInvite({ account: S.name, root: S.root.public, head: c.head, signer: laptop.id, sha256: share.sha256, inbox: c.state.inbox, epoch: c.state.epoch, wrap: wrap(c.state.inbox, key, context(c), label), invitation })), laptop.sign),
    ephemeral: b64(keyPair("x", `ephemeral/${label}`).seed),
  });
  /** What the recipient's device signs, when its history stood at `c`. */
  const binding = (c, signer) => ({ account: R.name, root: R.root.public, head: c.head, signer: signer.id, share_id: share.id, invite_id: invitation.id, sha256: share.sha256, owner: S.name, owner_root: S.root.public, inbox: c.state.inbox ?? "", epoch: c.state.epoch });
  const claimed = (value, signer, { kind = "recipient-claim", by = secret } = {}) => {
    const signed = Buffer.isBuffer(value) ? signBytes(kind, value, signer.sign) : sign(kind, value, signer.sign);
    return { ...signed, proof: bindingProof(by, signed) };
  };
  /** The grant for a claim, wrapped for the recipient as its history stands at `now`. */
  const grant = (claim, now, at, label) => ({
    statement: signBytes("named-grant", Buffer.from(writtenGrant({ account: S.name, root: S.root.public, head: at.head, signer: laptop.id, share_id: share.id, invite_id: invitation.id, sha256: share.sha256, recipient: R.name, recipient_root: R.root.public, inbox: now.state.inbox, epoch: now.state.epoch, wrap: wrap(now.state.inbox, key, context(now), label), claim_hash: hash(claim) })), laptop.sign),
    ephemeral: b64(keyPair("x", `ephemeral/${label}`).seed),
  });
  const base = (name, claim, now, { invited = sender, at = sender, keys = ["a"] } = {}) => {
    const made = invite(invited);
    return {
      name,
      share,
      key: b64(key),
      invitation: { ...invitation, secret: b64(secret) },
      owner: { account: S.name, root: S.root.public, head: at.head, signer: laptop.id, seed: b64(laptop.sign.seed), history: at.events, invited: invited.events.length, inboxes: inboxes(S, "a") },
      invite: made.statement,
      invite_ephemeral: made.ephemeral,
      claim,
      recipient: { account: R.name, history: now.events, inboxes: inboxes(R, ...keys) },
    };
  };
  const granted = (name, claim, now, options = {}) => {
    const made = grant(claim, now, options.at ?? sender, `share/grant/${name}`);
    push({ ...base(name, claim, now, options), expect: "granted", grant: made.statement, grant_ephemeral: made.ephemeral });
  };
  const refused = (name, claim, now = alone, { expect = "refused", why, ...options } = {}) => push({ ...base(name, claim, now, options), expect, ...(why ? { why } : {}) });

  /* What a sender grants. */
  const honest = claimed(binding(alone, phone), phone);
  granted("a claim from the recipient's only device", honest, alone);
  granted("a claim from the second of two devices", claimed(binding(two, tablet), tablet), two);
  granted("a recipient that approved a device after claiming", honest, two);
  const rotated = R.revoke(two, phone, tablet, "b");
  granted("a recipient that revoked a device after claiming, wrapped to its new key", claimed(binding(two, phone), phone), rotated, { keys: ["a", "b"] });
  granted("a claim by a device that was revoked since", claimed(binding(two, tablet), tablet), rotated, { keys: ["a", "b"] });
  granted("a sender that approved a device between inviting and granting", honest, alone, { at: S.approve(sender, laptop, desktop) });
  granted("a claim with a field no reader knows", claimed({ ...binding(alone, phone), note: "unknown" }, phone), alone);

  /* The proof. */
  refused("a proof made with another secret", claimed(binding(alone, phone), phone, { by: material("share/other secret") }));
  refused("another claim's proof", { ...honest, proof: claimed(binding(two, tablet), tablet).proof });
  refused("a proof one byte short", { ...honest, proof: b64(unb64(honest.proof).subarray(0, 31)) });
  refused("a proof with padding", { ...honest, proof: `${honest.proof}=` });
  refused("no proof", { ...honest, proof: "" });
  refused("the signature where the proof belongs", { ...honest, proof: honest.signature });

  /* What the claim is for. Each is signed by the device and proven with the secret. */
  for (const [what, change] of [
    ["another share", { share_id: "sharefxtvr2346" }],
    ["another invitation", { invite_id: b64(material("share/other invitation id", 24)) }],
    ["another ciphertext", { sha256: "ab".repeat(32) }],
    ["another sender", { owner: "usr_other" }],
    ["a sender with another root key", { owner_root: account("usr_other").root.public }],
  ]) {
    refused(`a claim for ${what}`, claimed({ ...binding(alone, phone), ...change }, phone));
  }

  /* Who signed it, and when. */
  const stranger = account("usr_other").device("phone", "Phone");
  refused("a claim signed by a device the history never held", claimed({ ...binding(alone, phone), signer: stranger.id }, stranger));
  refused("a claim signed by another device in the first one's name", claimed(binding(two, phone), tablet), two);
  refused("a claim signed as a grant", claimed(binding(alone, phone), phone, { kind: "named-grant" }));
  refused("a claim changed after it was signed", { ...honest, payload: b64(unb64(honest.payload).toString().replace(share.id, "sharefxtvr2346")) });
  refused("a claim by a device that had been revoked", claimed(binding(rotated, tablet), tablet), rotated, { keys: ["a", "b"] });
  refused("a claim by a device before it was approved", claimed({ ...binding(alone, phone), signer: tablet.id }, tablet), two);
  refused("a claim signed at a head the history does not hold", claimed({ ...binding(alone, phone), head: sender.head }, phone));
  refused("a claim that names a key the account did not have then", claimed({ ...binding(alone, phone), inbox: R.inbox("b") }, phone));
  refused("a claim that names the key of a later epoch", claimed({ ...binding(two, phone), inbox: R.inbox("b"), epoch: 2 }, phone), rotated, { keys: ["a", "b"] });
  refused("a claim at another epoch than the account was at", claimed({ ...binding(alone, phone), epoch: 2 }, phone));

  /* The recipient's history. */
  const before = R.init(phone);
  refused("a recipient whose history has lost its last event", claimed(binding(alone, phone), phone), before);
  refused("a recipient whose history is another account's", honest, { events: sender.events });
  refused("a recipient whose history starts at its second event", honest, { events: alone.events.slice(1) });
  refused("a recipient with no history", honest, { events: [] });
  refused("a claim under another root key than the history's", claimed({ ...binding(alone, phone), root: S.root.public }, phone));
  refused("a recipient that does not share", claimed(binding(before, phone), phone), before);

  /* The payload. */
  const text = JSON.stringify(binding(alone, phone));
  refused("a claim that is not JSON", claimed(Buffer.from("not json"), phone));
  refused("a claim that is a list of its values", claimed(Buffer.from(JSON.stringify(Object.values(binding(alone, phone)))), phone));
  refused("a claim with its epoch in quotes", claimed(Buffer.from(text.replace('"epoch":1', '"epoch":"1"')), phone));
  refused("a claim without an epoch", claimed(Buffer.from(text.replace(',"epoch":1', "")), phone));
  refused("a claim without a signer", claimed(Buffer.from(text.replace(/"signer":"[^"]*",/, "")), phone));
  const goOnly = { go: "granted", rust: "refused" };
  refused("a claim with a field's name in capitals", claimed(Buffer.from(text.replace('"owner":', '"OWNER":')), phone), alone, {
    expect: goOnly,
    why: "Go matches a field's name without regard to case. Rust refuses a payload the viewer and the service would read differently.",
  });
  refused("a claim with a field given twice", claimed(Buffer.from(`{"epoch":7,${text.slice(1)}`), phone), alone, {
    expect: goOnly,
    why: "Go takes the last of two. Rust refuses a payload that says a thing twice.",
  });
  refused("a claim made before the recipient shared, naming no key", claimed(binding(before, phone), phone), alone, {
    expect: goOnly,
    why: "Go reads an account with no incoming-share key as having the empty one, which a claim can then name. The service refuses a claim from an account that does not share, and Rust reads no key as none.",
  });
  return shares;
}

/** What a private key and a point agree on, or nothing where the library refuses the point. */
function agreed(key, point) {
  try {
    return diffieHellman({ privateKey: key.private, publicKey: publicKey("x", point) });
  } catch {
    return null;
  }
}
const secret0 = (key, half) => diffieHellman({ privateKey: key.private, publicKey: publicKey("x", unb64(half)) });

/**
 * The limits. They take long histories to reach, a megabyte of them, so they
 * are not written to a file: scripts/protocol-diff.mjs builds them and hands
 * them to every implementation.
 */
export function limitCases() {
  const out = collector();
  const A = account("usr_fixture");
  const laptop = A.device("laptop", "Laptop");
  const phone = A.device("phone", "Phone");
  let long = A.init(laptop);
  while (long.events.length < 511) long = long.events.length % 2 ? A.approve(long, laptop, phone) : A.revoke(long, laptop, phone);
  const full = A.approve(long, laptop, phone);
  out.valid("512 events", full);
  out.invalid("513 events", "length", A.revoke(full, laptop, phone).events);

  let crowd = A.init(laptop);
  for (let i = 2; i <= 32; i++) crowd = A.approve(crowd, laptop, A.device(`crowd ${i}`, `Device ${i}`));
  out.valid("32 devices", crowd);
  out.invalid("33 devices", "devices", A.approve(crowd, laptop, A.device("crowd 33", "Device 33")).events);
  return { signed: out.signed, cases: out.cases };
}

/* ----------------------------------------------------------------- files */

// Only ASCII reaches the files, so that nothing invisible can sit in one.
const ascii = (text) => text.replace(/[^\x20-\x7e]/g, (c) => `${backslash}u${c.charCodeAt(0).toString(16).padStart(4, "0")}`);
const line = (value) => ascii(JSON.stringify(value));
const list = (items) => `[\n${items.map(line).join(",\n")}\n]\n`;
const table = ({ signed, cases }) =>
  `{\n"signed": {\n${Object.keys(signed).sort().map((id) => `${JSON.stringify(id)}: ${line(signed[id])}`).join(",\n")}\n},\n"cases": [\n${cases.map(line).join(",\n")}\n]\n}\n`;

const sections = (value) =>
  `{\n${Object.entries(value).map(([name, items]) => `${JSON.stringify(name)}: [\n${items.map(line).join(",\n")}\n]`).join(",\n")}\n}\n`;

export function files() {
  return {
    "envelopes.json": list(envelopeCases()),
    "statements.json": list(statementCases()),
    "histories.json": table(historyCases()),
    "keys.json": sections(keyCases()),
  };
}

if (import.meta.url === pathToFileURL(process.argv[1] ?? "").href) {
  const mode = process.argv[2];
  if (mode !== "--check" && mode !== "--update") {
    console.error("usage: node scripts/protocol-cases.mjs --check | --update");
    process.exit(2);
  }
  let stale = 0;
  for (const [name, text] of Object.entries(files())) {
    const file = path.join(directory, name);
    let current = null;
    try {
      current = readFileSync(file, "utf8");
    } catch {}
    if (current === text) continue;
    if (mode === "--update") {
      writeFileSync(file, text);
      console.log(`wrote ${path.relative(root, file)}`);
    } else {
      stale++;
      console.error(`${path.relative(root, file)} is not what scripts/protocol-cases.mjs writes; run it with --update`);
    }
  }
  if (stale) process.exit(1);
  if (mode === "--check") console.log("testdata/protocol matches scripts/protocol-cases.mjs");
}
