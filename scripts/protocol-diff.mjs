#!/usr/bin/env node
/**
 * Differential check of the protocol verifiers (docs/rust-migration.md, P1).
 * It makes envelopes, signed statements, device histories and vault records
 * that are wrong in one way each, hands the same bytes to every
 * implementation, and compares what each makes of them:
 *
 *   js      the viewer's code on Node's Web Crypto (packages/viewer)
 *   go      the Go CLI, through TestSweep in go/internal/identity
 *   rust    crates/slink-protocol, through its `protocol-verdicts` example
 *   server  the hosted service's verifier, when its checkout is named
 *
 *   node scripts/protocol-diff.mjs                    run everything
 *   node scripts/protocol-diff.mjs --list             every difference, not a summary
 *   node scripts/protocol-diff.mjs <text>             only the cases whose name contains <text>
 *   node scripts/protocol-diff.mjs --server <dir>     also run lib/identity-store.mjs from <dir>
 *
 * VERDICTS_RUST names a prebuilt helper; without it the script runs
 * `cargo run`. The Go side always runs through `go test`.
 *
 * An envelope `opens` or is `refused`, a statement is `verified` or
 * `refused`, a history or record is `valid` or `invalid`. Where every
 * implementation accepts, they must also have read the same thing: the same
 * document, or the same state at the same head.
 *
 * Verdicts that differ must be one of the kinds listed under `explained`
 * below, each of which testdata/protocol/histories.json pins with a case.
 * In all of them Rust is among those that refuse; a case that Rust accepts
 * and another implementation refuses fails the run whatever its kind.
 *
 * The cases in testdata/protocol are the contract and run in each language's
 * own tests. This sweep is how a difference nobody wrote a case for is found;
 * when it finds one, the case goes there.
 */
import { spawnSync } from "node:child_process";
import { closeSync, mkdtempSync, openSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath, pathToFileURL } from "node:url";
import { build } from "esbuild";
import { account, b64, digest, envelope, hash, keyPair, limitCases, material, sign, signBytes, summary, unb64 } from "./protocol-cases.mjs";

const started = Date.now();
const root = path.join(path.dirname(fileURLToPath(import.meta.url)), "..");
const args = process.argv.slice(2);
const list = args.includes("--list");
const serverDir = args.includes("--server") ? args[args.indexOf("--server") + 1] : null;
const filter = args.find((arg, index) => !arg.startsWith("--") && args[index - 1] !== "--server");

/* ----------------------------------------------------------------- cases */

const cases = [];
const add = (name, item) => cases.push({ name, ...item });
const history = (name, events, extra = {}) => add(name, { t: "history", account: "usr_fixture", events, ...extra });

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
const CHAINS = { first, shared, approved, recovered, revoked, three, sharedTwo, rotated };
for (const [name, c] of Object.entries(CHAINS)) history(`${name} as written`, c.events);

/* One field of the last event changed, the event signed again by its signer. */

const backslash = String.fromCharCode(0x5c);
const VALUES = ["null", "true", "false", "0", "1", "-1", "1.5", '""', '"x"', "[]", "{}", "[0]", '{"a":1}'];
const numbers = (n) => [`${n}.0`, `${n}e0`, `${n}E0`, `${n}.00`, `0${n}`, `+${n}`, `-${n}`, ` ${n} `, `"${n}"`, `${n + 1}`, `${n - 1}`, `${n}0e-1`, "9007199254740991", "9007199254740992", "9007199254740993", "18446744073709551616", "1e400", "1e2", "0x1"];
const ALPHABET = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";
const strings = (text) => [
  `${text}A`,
  text.slice(0, -1),
  text.slice(1),
  text.toUpperCase(),
  text.toLowerCase(),
  text.length ? text.slice(0, -1) + ALPHABET[(ALPHABET.indexOf(text.at(-1)) + 1) % 64] : "A",
  text.replaceAll("-", "+").replaceAll("_", "/"),
  `${text}=`,
  `${text}\n`,
  ` ${text}`,
  `${text}\u0000`,
  `${text}.`,
].map((value) => JSON.stringify(value));
const names = (field) => [
  field.toUpperCase(),
  field[0].toUpperCase() + field.slice(1),
  field.replace("s", String.fromCodePoint(0x17f)),
  field.replace("k", String.fromCodePoint(0x212a)),
  `${field} `,
  ` ${field}`,
  `${field}s`,
  field.slice(0, -1),
].filter((name) => name !== field).map((name) => JSON.stringify(name)).concat(`"${backslash}u00${field.charCodeAt(0).toString(16)}${field.slice(1)}"`);

/**
 * Every single change to one object's fields, as [what, text of the object].
 * `text` is the object's JSON and `value` the object.
 */
function* changes(value) {
  const entries = Object.entries(value);
  const write = (list) => `{${list.map(([key, token]) => `${key}:${token}`).join(",")}}`;
  const tokens = entries.map(([key, v]) => [JSON.stringify(key), JSON.stringify(v)]);
  for (let i = 0; i < entries.length; i++) {
    const [field, v] = entries[i];
    const [key, token] = tokens[i];
    const put = (replacement) => write(tokens.map((pair, at) => (at === i ? replacement : pair)));
    yield [`${field} left out`, write(tokens.filter((_, at) => at !== i))];
    for (const other of VALUES) if (other !== token) yield [`${field} as ${other}`, put([key, other])];
    if (typeof v === "number") for (const other of numbers(v)) yield [`${field} written ${other}`, put([key, other])];
    if (typeof v === "string") for (const other of strings(v)) if (other !== token) yield [`${field} as ${other.length > 40 ? `${other.slice(0, 24)}...${other.slice(-12)}` : other}`, put([key, other])];
    for (const other of names(field)) yield [`${field} named ${other}`, put([other, token])];
    yield [`${field} twice, the same`, write([...tokens, [key, token]])];
    yield [`${field} twice, null first`, write([[key, "null"], ...tokens])];
    yield [`${field} twice, null last`, write([...tokens, [key, "null"]])];
    const capital = JSON.stringify(field[0].toUpperCase() + field.slice(1));
    yield [`${field} again as ${capital}, the same`, write([...tokens, [capital, token]])];
    yield [`${field} again as ${capital}, null`, write([...tokens, [capital, "null"]])];
    yield [`${field} again as ${capital}, before it`, write([[capital, '"x"'], ...tokens])];
  }
  yield ["an unknown field", write([...tokens, ['"unknown"', '{"a":[1,null]}']])];
  yield ["an empty field name", write([...tokens, ['""', "1"]])];
}

for (const [name, c] of Object.entries(CHAINS)) {
  const state = c.state;
  const prefix = c.events.slice(0, -1);
  const key = A.keyOf(state.signer);
  const signed = (text) => [...prefix, signBytes("event", Buffer.from(text), key)];
  for (const [what, text] of changes(state)) history(`${name}: ${what}`, signed(text));
  // The same for the fields of each device, inside the state.
  state.devices.forEach((device, at) => {
    for (const [what, text] of changes(device)) {
      const marker = "\u0000device\u0000";
      const devices = state.devices.map((d, i) => (i === at ? marker : d));
      history(`${name}: device ${at}: ${what}`, signed(JSON.stringify({ ...state, devices }).replace(JSON.stringify(marker), text)));
    }
  });
  if (state.inbox === undefined) {
    const text = JSON.stringify(state);
    for (const other of [...VALUES, JSON.stringify(A.inbox("a")), JSON.stringify(b64(material("short", 31)))]) {
      history(`${name}: inbox added as ${other}`, signed(`{"inbox":${other},${text.slice(1)}`));
      history(`${name}: inbox added as ${other}, then null`, signed(`{"inbox":${other},${text.slice(1, -1)},"inbox":null}`));
      history(`${name}: inbox added as null, then ${other}`, signed(`{"inbox":null,${text.slice(1, -1)},"inbox":${other}}`));
    }
  }
  history(`${name}: an unknown field that repeats a key`, signed(JSON.stringify({ ...state, future: 0 }).replace('"future":0', '"future":{"a":1,"a":2}')));
  history(`${name}: an unknown field twice`, signed(`{"future":1,${JSON.stringify(state).slice(1, -1)},"future":2}`));
}

/* The payload as text. */

for (const [name, c] of Object.entries({ first, approved })) {
  const text = JSON.stringify(c.state);
  const prefix = c.events.slice(0, -1);
  const key = A.keyOf(c.state.signer);
  const signed = (bytes) => [...prefix, signBytes("event", Buffer.from(bytes), key)];
  const nested = (depth) => text.replace("{", `{"deep":${"[".repeat(depth)}${"]".repeat(depth)},`);
  const within = (depth) => text.replace('"devices":[', `"devices":${"[".repeat(depth)}`).replace(/\]\}$/, `${"]".repeat(depth)}}`);
  const TEXTS = {
    "a byte-order mark first": Buffer.concat([Buffer.from([0xef, 0xbb, 0xbf]), Buffer.from(text)]),
    "a UTF-16 byte-order mark first": Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from(text)]),
    "white space around": ` \t\r\n${text} \t\r\n`,
    "a form feed after": `${text}\f`,
    "a no-break space after": `${text}${String.fromCodePoint(0xa0)}`,
    "a null character after": `${text}\u0000`,
    "text after": `${text}x`,
    "a second value after": `${text}{}`,
    "a comma after": `${text},`,
    "the last brace missing": text.slice(0, -1),
    "a trailing comma": text.replace(/\}$/, ",}"),
    "a comment first": `// c\n${text}`,
    "single quotes": text.replaceAll('"', "'"),
    "an unquoted key": text.replace('"account"', "account"),
    "wrapped in a list": `[${text}]`,
    "wrapped in an object": `{"state":${text}}`,
    "as a JSON string": JSON.stringify(text),
    "as a list of its values": JSON.stringify(Object.values(c.state)),
    "devices as lists of their values": JSON.stringify({ ...c.state, devices: c.state.devices.map((d) => Object.values(d)) }),
    "NaN in an unknown field": text.replace("{", '{"n":NaN,'),
    "a number too large for a double in an unknown field": text.replace("{", '{"n":1e400,'),
    "an integer of 30 digits in an unknown field": text.replace("{", '{"n":123456789012345678901234567890,'),
    "a leading zero in an unknown field": text.replace("{", '{"n":01,'),
    "half a surrogate pair in an unknown field": text.replace("{", `{"n":"${backslash}ud83d",`),
    "a reversed surrogate pair in an unknown field": text.replace("{", `{"n":"${backslash}ude00${backslash}ud83d",`),
    "a whole surrogate pair in an unknown field": text.replace("{", `{"n":"${backslash}ud83d${backslash}ude00",`),
    "half a surrogate pair in a device name": text.replace('"name":"', `"name":"${backslash}ud83d`),
    "an escaped null character in a device name": text.replace('"name":"', `"name":"${backslash}u0000`),
    "a raw tab in a device name": text.replace('"name":"', '"name":"\t'),
    "an unknown escape in a device name": text.replace('"name":"', `"name":"${backslash}x41`),
    "a byte that is not UTF-8 in a device name": Buffer.from(text.replace('"name":"', '"name":"\u0000')).map((byte) => (byte === 0 ? 0xff : byte)),
    "a byte that is not UTF-8 in an unknown field": Buffer.from(text.replace("{", '{"n":"\u0000",')).map((byte) => (byte === 0 ? 0xff : byte)),
    "a byte that is not UTF-8 in a field name": Buffer.from(text.replace("{", '{"\u0000":1,')).map((byte) => (byte === 0 ? 0xc0 : byte)),
    "an overlong encoding in a device name": Buffer.concat([Buffer.from(text.split('"name":"')[0] + '"name":"'), Buffer.from([0xc0, 0xaf]), Buffer.from(text.split('"name":"').slice(1).join('"name":"'))]),
    "a surrogate encoded as UTF-8 in a device name": Buffer.concat([Buffer.from(text.split('"name":"')[0] + '"name":"'), Buffer.from([0xed, 0xa0, 0x80]), Buffer.from(text.split('"name":"').slice(1).join('"name":"'))]),
    ...Object.fromEntries([1, 64, 120, 126, 127, 128, 129, 200, 1000, 5000].map((depth) => [`an unknown field nested ${depth} deep`, nested(depth)])),
    ...Object.fromEntries([2, 127, 128, 200].map((depth) => [`devices nested ${depth} deep`, within(depth)])),
    ...Object.fromEntries(
      [80, 81].flatMap((bytes) => [[1, 0x6e], [2, 0xe9], [4, 0x1f4bb]].map(([width, point]) => [
        `a device name of ${Math.floor(bytes / width)} characters of ${width} bytes and ${bytes % width} letters`,
        text.replace('"name":"Laptop"', `"name":${JSON.stringify(String.fromCodePoint(point).repeat(Math.floor(bytes / width)) + "n".repeat(bytes % width))}`),
      ])),
    ),
  };
  for (const [what, bytes] of Object.entries(TEXTS)) history(`${name}: ${what}`, signed(bytes));
  // Around the size the hosted service stops at: 32 KiB of payload as text.
  for (const length of [32766, 32767, 32768, 32769, 32770, 40000]) {
    const bytes = Math.floor((length * 3) / 4);
    const padded = text.replace("{", `{"padding":"${"p".repeat(bytes - Buffer.byteLength(text) - 13)}",`);
    history(`${name}: a payload of ${b64(Buffer.from(padded)).length} characters`, signed(padded));
  }
}

/* Every next event a state could be given, in whatever combination. */

const product = (...lists) => lists.reduce((rows, items) => rows.flatMap((row) => items.map((item) => [...row, item])), [[]]);
for (const [name, base] of Object.entries({ approved, sharedTwo, three })) {
  const before = base.state;
  const rewrap = (list, epoch, keep = -1) => list.map((d, i) => (i === keep ? d : A.entry([laptop, phone, tablet, desktop].find((known) => known.id === d.id), epoch)));
  const DEVICES = {
    same: () => before.devices,
    "one more": (epoch) => [...before.devices, A.entry(desktop, epoch)],
    "two more": (epoch) => [...before.devices, A.entry(desktop, epoch), A.entry(A.device("extra"), epoch)],
    "the last removed": () => before.devices.slice(0, -1),
    "the first removed": () => before.devices.slice(1),
    "the last replaced": (epoch) => [...before.devices.slice(0, -1), A.entry(desktop, epoch)],
    reversed: () => [...before.devices].reverse(),
    "wrapped again": (epoch) => rewrap(before.devices, epoch),
    "wrapped again but for the first": (epoch) => rewrap(before.devices, epoch, 0),
    "the last removed and the rest wrapped again": (epoch) => rewrap(before.devices.slice(0, -1), epoch),
    "the first removed and the rest wrapped again": (epoch) => rewrap(before.devices.slice(1), epoch),
    "the last removed, the rest wrapped again but for the first": (epoch) => rewrap(before.devices.slice(0, -1), epoch, 0),
    "the last replaced and the rest wrapped again": (epoch) => [...rewrap(before.devices.slice(0, -1), epoch), A.entry(desktop, epoch)],
  };
  const INBOX = { kept: before.inbox, none: undefined, "a new key": A.inbox("z"), "the first key": A.inbox("a") };
  const SIGNERS = { "the first device": [laptop.id, laptop.sign], "the root key": ["root", A.root], "the last device": [before.devices.at(-1).id, A.keyOf(before.devices.at(-1).id)] };
  for (const [kind, [who, [signer, key]], epoch, [inboxName, inbox], [devicesName, devices], recoveryWrap] of product(
    ["approve", "recover", "revoke", "sharing"],
    Object.entries(SIGNERS),
    [0, 1, 2],
    Object.entries(INBOX),
    Object.entries(DEVICES),
    ["kept", "new"],
  )) {
    const at = before.epoch + epoch;
    const { inbox: _, ...rest } = before;
    const state = {
      ...(inbox === undefined ? {} : { inbox }),
      ...rest,
      seq: before.seq + 1,
      prev: base.head,
      kind,
      signer,
      epoch: at,
      recovery_wrap: recoveryWrap === "kept" ? before.recovery_wrap : A.recoveryWrap(at + 7),
      devices: devices(at + 7),
    };
    history(`${name} then ${kind} by ${who}, epoch +${epoch}, inbox ${inboxName}, devices ${devicesName}, recovery wrapper ${recoveryWrap}`, [...base.events, sign("event", state, key)]);
  }
}

/* Who signs, as what, in whose name. */

for (const [name, c] of Object.entries(CHAINS)) {
  const prefix = c.events.slice(0, -1);
  for (const [kind, signer, [who, key], as] of product(
    ["init", "approve", "recover", "revoke", "sharing", "rename", ""],
    ["root", laptop.id, phone.id, tablet.id, "", "ROOT"],
    Object.entries({ root: A.root, laptop: laptop.sign, phone: phone.sign, tablet: tablet.sign, stranger: keyPair("ed", "stranger") }),
    ["event", "vault", "request"],
  )) {
    if (as !== "event" && (kind !== c.state.kind || signer !== c.state.signer)) continue;
    history(`${name} as ${kind || "no kind"} in the name of ${signer.slice(0, 8) || "no one"}, signed by ${who} as ${as}`, [...prefix, sign(as, { ...c.state, kind, signer }, key)]);
  }
}

/* The events as a list. */

history("no events", []);
history("the first event twice", [first.events[0], first.events[0]]);
history("the second event alone", [approved.events[1]]);
history("the second event first", [approved.events[1], approved.events[0]]);
history("the second event twice", [approved.events[0], approved.events[1], approved.events[1]]);
history("another history's second event", [first.events[0], shared.events[1], approved.events[1]]);
for (const of of ["usr_other", "", "USR_FIXTURE", "usr_fixture ", "usr_fixtur"]) history(`verified for the account ${JSON.stringify(of)}`, approved.events, { account: of });
for (const item of limitCases().cases) {
  const { signed, cases: _ } = limits();
  history(`limit: ${item.name}`, item.events.map((id) => signed[id]));
}
function limits() {
  limits.cache ??= limitCases();
  return limits.cache;
}

/* Vault records. */

const RECORDS = { first: [first, laptop], approved: [approved, phone], revoked: [revoked, laptop], recovered: [recovered, "root"], shared: [shared, laptop] };
for (const [name, [c, writer]] of Object.entries(RECORDS)) {
  const good = A.vault(c, writer, 4);
  history(`record ${name} as written`, c.events, { vault: good });
  const value = JSON.parse(unb64(good.payload).toString());
  const key = writer === "root" ? A.root : writer.sign;
  for (const [what, text] of changes(value)) history(`record ${name}: ${what}`, c.events, { vault: signBytes("vault", Buffer.from(text), key) });
  for (const [signer, [who, by], as] of product(
    ["root", laptop.id, phone.id, tablet.id, ""],
    Object.entries({ root: A.root, laptop: laptop.sign, phone: phone.sign, stranger: keyPair("ed", "stranger") }),
    ["vault", "event"],
  )) {
    history(`record ${name}: vault in the name of ${signer.slice(0, 8) || "no one"}, signed by ${who} as ${as}`, c.events, { vault: sign(as, { ...value, signer }, by) });
  }
  for (const other of Object.values(CHAINS)) history(`record ${name}: vault signed against ${other.head.slice(0, 8)}`, c.events, { vault: sign("vault", { ...value, head: other.head }, key) });
  for (const length of [0, 27, 28, 29, 4096]) history(`record ${name}: ${length} bytes of ciphertext`, c.events, { vault: sign("vault", { ...value, data: b64(material("data", length)) }, key) });
  history(`record ${name}: an event where the vault belongs`, c.events, { vault: c.events.at(-1) });
  history(`record ${name}: another record's vault`, c.events, { vault: A.vault(c === first ? approved : first, laptop, 4) });
}
// At the size limit of the ciphertext, which is counted as text.
for (const length of [6 * 1024 * 1024, 6 * 1024 * 1024 + 4]) {
  const value = { ...JSON.parse(unb64(A.vault(first, laptop, 4).payload).toString()), data: "A".repeat(length) };
  history(`record first: ciphertext of ${length} characters`, first.events, { vault: sign("vault", value, laptop.sign) });
}

/* Signed text, one character at a time. */

for (const [name, c] of Object.entries({ first, approved })) {
  const prefix = c.events.slice(0, -1);
  const last = c.events.at(-1);
  for (const field of ["payload", "signature"]) {
    const text = last[field];
    for (let at = 0; at < text.length; at += field === "payload" ? 5 : 1) {
      const swapped = text.slice(0, at) + (text[at] === "A" ? "B" : "A") + text.slice(at + 1);
      history(`${name}: ${field} character ${at} changed`, [...prefix, { ...last, [field]: swapped }]);
    }
    for (const [what, value] of [
      ["with padding", `${text}=`],
      ["with two padding characters", `${text}==`],
      ["in the other alphabet", text.replaceAll("-", "+").replaceAll("_", "/")],
      ["followed by a line break", `${text}\n`],
      ["with a line break inside", `${text.slice(0, 8)}\n${text.slice(8)}`],
      ["with a carriage return inside", `${text.slice(0, 8)}\r${text.slice(8)}`],
      ["with a space inside", `${text.slice(0, 8)} ${text.slice(8)}`],
      ["one character short", text.slice(0, -1)],
      ["one character long", `${text}A`],
      ["four characters short", text.slice(0, -4)],
      ["empty", ""],
      ["twice", text + text],
    ]) {
      history(`${name}: ${field} ${what}`, [...prefix, { ...last, [field]: value }]);
    }
  }
}

/* Statements: every kind against every kind, and keys that are not keys. */

const KINDS = ["event", "vault", "request", "recovery-check", "named-invite", "recipient-claim", "named-grant"];
const statementKey = keyPair("ed", "statement/key");
for (const [made, offered] of product(KINDS, KINDS)) {
  add(`a ${made} statement offered as ${offered}`, { t: "statement", kind: offered, public: statementKey.public, ...sign(made, { statement: "fixture" }, statementKey) });
}
{
  const good = sign("event", { statement: "fixture" }, statementKey);
  const order = (1n << 252n) + 27742317777372353535851937790883648493n;
  const raw = unb64(good.signature);
  const le = (bytes) => BigInt(`0x${Buffer.from(bytes).reverse().toString("hex")}`);
  const bytesOf = (number) => Buffer.from(number.toString(16).padStart(64, "0"), "hex").reverse();
  for (const k of [1n, 2n, 3n, 8n]) {
    const scalar = le(raw.subarray(32)) + k * order;
    if (scalar < 1n << 256n) add(`a signature with ${k} times the group order added`, { t: "statement", kind: "event", public: statementKey.public, payload: good.payload, signature: b64(Buffer.concat([raw.subarray(0, 32), bytesOf(scalar)])) });
  }
  // The eight points of small order and their other spellings, as keys and
  // as the point in the signature, each on a few payloads.
  const field = (1n << 255n) - 19n;
  const POINTS = {
    "the identity": 1n,
    "the point of order two": field - 1n,
    "a point of order four": 0n,
    "a point of order eight": 0x7a03ac9277fdc74ec6cc392cfa53202a0f67100d760b3cba4fd84d3d706a17c7n,
    "another point of order eight": 0x05fc536d880238b13933c6d305acdfd5f098eff289f4c345b027b2c28f95e826n,
  };
  for (const [what, y] of Object.entries(POINTS)) {
    for (const [spelling, value] of [["", y], [" with the sign bit", y | (1n << 255n)], [" past the field's size", y + field], [" past the field's size with the sign bit", (y + field) | (1n << 255n)]]) {
      if (value >> 256n) continue;
      if (spelling.includes("past") && y + field >= 1n << 255n) continue;
      const point = bytesOf(value);
      for (let n = 0; n < 6; n++) {
        const payload = b64(JSON.stringify({ statement: "fixture", n }));
        add(`${what}${spelling} as a key, payload ${n}`, { t: "statement", kind: "event", public: b64(point), payload, signature: b64(Buffer.concat([bytesOf(1n), Buffer.alloc(32)])) });
        add(`${what}${spelling} as a key and in the signature, payload ${n}`, { t: "statement", kind: "event", public: b64(point), payload, signature: b64(Buffer.concat([point, Buffer.alloc(32)])) });
      }
      add(`${what}${spelling} in a good key's signature`, { t: "statement", kind: "event", public: statementKey.public, payload: good.payload, signature: b64(Buffer.concat([point, raw.subarray(32)])) });
    }
  }
  for (const [what, key] of [["32 zero bytes", Buffer.alloc(32)], ["32 bytes of ones", Buffer.alloc(32, 0xff)], ["a number that is no point", bytesOf(2n)]]) {
    add(`${what} as a key`, { t: "statement", kind: "event", public: b64(key), ...good });
  }
}

/* Envelopes: every byte, every length, every character of the key. */

const ENVELOPES = {
  "Web Crypto": JSON.parse(readFileSync(path.join(root, "testdata/encryption/webcrypto.json"), "utf8")),
  Go: JSON.parse(readFileSync(path.join(root, "testdata/encryption/go.json"), "utf8")),
};
for (const [source, fixture] of Object.entries(ENVELOPES)) {
  const data = Buffer.from(fixture.envelope, "base64");
  const item = (bytes, key = fixture.key) => ({ t: "envelope", envelope: Buffer.from(bytes).toString("base64"), key });
  add(`${source} envelope as written`, item(data));
  for (let at = 0; at < data.length; at++) {
    for (const bit of [1, 0x80]) {
      const changed = Buffer.from(data);
      changed[at] ^= bit;
      add(`${source} envelope: byte ${at} xor ${bit}`, item(changed));
    }
  }
  for (let length = 0; length < data.length; length++) add(`${source} envelope: the first ${length} bytes`, item(data.subarray(0, length)));
  for (const extra of [1, 16, 24]) add(`${source} envelope: ${extra} bytes added`, item(Buffer.concat([data, Buffer.alloc(extra)])));
  for (let at = 0; at < fixture.key.length; at++) {
    add(`${source} envelope: key character ${at} changed`, item(data, fixture.key.slice(0, at) + (fixture.key[at] === "A" ? "B" : "A") + fixture.key.slice(at + 1)));
  }
  for (const [what, key] of [["with padding", `${fixture.key}=`], ["one short", fixture.key.slice(0, -1)], ["one long", `${fixture.key}A`], ["in the other alphabet", fixture.key.replaceAll("-", "+").replaceAll("_", "/")], ["followed by a line break", `${fixture.key}\n`], ["empty", ""], ["of 43 dots", ".".repeat(43)]]) {
    add(`${source} envelope: a key ${what}`, item(data, key));
  }
}
for (const size of [0, 1, 15, 16, 17, 4096]) {
  const sealed = envelope(material("document", size), `sweep ${size}`);
  add(`an envelope of ${size} bytes of document`, { t: "envelope", envelope: sealed.data.toString("base64"), key: sealed.key });
  for (const [what, header] of Object.entries({ "version 0": { magic: [0x53, 0x4c, 0x49, 0x4e, 0x4b, 0x45, 0, 0] }, "version 2": { magic: [0x53, 0x4c, 0x49, 0x4e, 0x4b, 0x45, 0, 2] }, "lower-case magic": { magic: [0x73, 0x6c, 0x69, 0x6e, 0x6b, 0x65, 0, 1] }, "a reserved byte set": { reserved: [0, 1, 0, 0] } })) {
    const other = envelope(material("document", size), `sweep ${size} ${what}`, header);
    add(`an envelope of ${size} bytes under ${what}`, { t: "envelope", envelope: other.data.toString("base64"), key: other.key });
  }
}

/* ------------------------------------------------------- implementations */

const selected = filter ? cases.filter((item) => item.name.includes(filter)) : cases;
if (!selected.length) {
  console.error(`no case matches ${JSON.stringify(filter)}`);
  process.exit(2);
}

const bundled = await build({
  stdin: { contents: 'export * from "./identity-protocol"; export * from "./identity-crypto"; export * from "./encryption";', resolveDir: path.join(root, "packages/viewer"), loader: "ts" },
  bundle: true,
  write: false,
  format: "esm",
  logLevel: "silent",
});
const viewer = await import(`data:text/javascript;base64,${Buffer.from(bundled.outputFiles[0].text).toString("base64")}`);
const refusal = (verdict, error) => ({ verdict, error: String(error?.message ?? error) });
const read = (state, head, vault) => ({
  head,
  seq: state.seq + 0,
  kind: state.kind,
  signer: state.signer,
  epoch: state.epoch,
  inbox: state.inbox || "",
  devices: state.devices.map((device) => ({ id: device.id, name: device.name })),
  ...(vault ? { revision: vault.revision } : {}),
});
async function judgeJs(item) {
  try {
    if (item.t === "envelope") {
      return { verdict: "opens", plaintext: Buffer.from(await viewer.decryptDocument(new Uint8Array(Buffer.from(item.envelope, "base64")), item.key)).toString("base64") };
    }
    if (item.t === "statement") {
      await viewer.verify(item.kind, { payload: item.payload, signature: item.signature }, item.public);
      return { verdict: "verified" };
    }
    const { state, head, vault } = item.vault ? await viewer.verifyRecord({ events: item.events, vault: item.vault }, item.account) : await viewer.verifyHistory(item.events, item.account);
    return { verdict: "valid", state: read(state, head, vault) };
  } catch (error) {
    return refusal(item.t === "history" ? "invalid" : "refused", error);
  }
}

// The hosted service's own verifier. It has no envelope code: it stores
// ciphertext and never opens it.
const server = serverDir ? await import(pathToFileURL(path.resolve(serverDir, "lib/identity-store.mjs")).href) : null;
function judgeServer(item) {
  if (item.t === "envelope") return null;
  try {
    if (item.t === "statement") {
      server.verifySigned({ payload: item.payload, signature: item.signature }, item.kind, item.public);
      return { verdict: "verified" };
    }
    const { state, head, vault } = item.vault ? server.verifyRecord({ events: item.events, vault: item.vault }, item.account) : server.verifyHistory(item.events, item.account);
    return { verdict: "valid", state: read(state, head, vault) };
  } catch (error) {
    return refusal(item.t === "history" ? "invalid" : "refused", error);
  }
}

const scratch = mkdtempSync(path.join(os.tmpdir(), "protocol-diff-"));
const input = path.join(scratch, "input.jsonl");
function answers(name, file) {
  const lines = readFileSync(file, "utf8").trimEnd().split("\n");
  if (lines.length !== selected.length) {
    console.error(`the ${name} helper answered ${lines.length} of ${selected.length} cases`);
    process.exit(2);
  }
  return lines.map((line) => JSON.parse(line));
}
function failed(name, run) {
  if (!run.error && run.status === 0) return;
  console.error(`the ${name} helper failed${run.error ? `: ${run.error.message}` : ` with status ${run.status}`}`);
  process.exit(2);
}
function runGo() {
  const output = path.join(scratch, "go.jsonl");
  const run = spawnSync("go", ["test", "-count=1", "-run", "^TestSweep$", "./internal/identity"], {
    cwd: path.join(root, "go"),
    env: { ...process.env, SLINK_PROTOCOL_SWEEP: input, SLINK_PROTOCOL_VERDICTS: output },
    stdio: ["ignore", "ignore", "inherit"],
  });
  failed("go", run);
  return answers("go", output);
}
function runRust() {
  const output = path.join(scratch, "rust.jsonl");
  const [stdin, stdout] = [openSync(input, "r"), openSync(output, "w")];
  const prebuilt = process.env.VERDICTS_RUST;
  const run = prebuilt
    ? spawnSync(prebuilt, [], { stdio: [stdin, stdout, "inherit"] })
    : spawnSync("cargo", ["run", "--release", "--locked", "--quiet", "-p", "slink-protocol", "--example", "protocol-verdicts"], { cwd: root, stdio: [stdin, stdout, "inherit"] });
  closeSync(stdin);
  closeSync(stdout);
  failed("rust", run);
  return answers("rust", output);
}

let results;
const clock = (label, since) => process.env.PROTOCOL_DIFF_TIMES && console.error(`${label}: ${Date.now() - since} ms`);
clock("cases built", started);
try {
  writeFileSync(input, selected.map(({ name, ...item }) => JSON.stringify(item)).join("\n") + "\n");
  let since = Date.now();
  const go = runGo();
  clock("go", since);
  since = Date.now();
  const rust = runRust();
  clock("rust", since);
  since = Date.now();
  // Web Crypto works on other threads, so the viewer's side runs a batch at
  // a time.
  results = [];
  for (let from = 0; from < selected.length; from += 256) {
    const batch = selected.slice(from, from + 256);
    const js = await Promise.all(batch.map(judgeJs));
    batch.forEach((item, at) => results.push({ ...item, js: js[at], go: go[from + at], rust: rust[from + at], server: server ? judgeServer(item) : null }));
  }
  clock("js", since);
} finally {
  rmSync(scratch, { recursive: true, force: true });
}

/* ------------------------------------------------------------ comparison */

const accepts = (side) => side.verdict === "valid" || side.verdict === "verified" || side.verdict === "opens";
const payloadOf = (result) => {
  const last = result.vault ?? result.events?.at(-1);
  return last ? Buffer.from(last.payload ?? "", "base64url") : Buffer.alloc(0);
};
const text = (result) => payloadOf(result).toString("latin1");
const FIELDS = ["inbox", "account", "seq", "prev", "kind", "signer", "root", "recovery", "recovery_box", "recovery_wrap", "epoch", "devices", "id", "name", "sign", "box", "wrap", "revision", "head", "data"];
const fold = (key) => [...key].map((c) => (c === String.fromCodePoint(0x17f) ? "s" : c === String.fromCodePoint(0x212a) ? "k" : c.toLowerCase())).join("");
/** Field names in the payload that are a known field in another case. */
function folded(result) {
  const keys = [...payloadOf(result).toString("utf8").matchAll(/"((?:[^"\\]|\\.)*)"\s*:/g)].map((match) => {
    try {
      return JSON.parse(`"${match[1]}"`);
    } catch {
      return match[1];
    }
  });
  return keys.some((key) => !FIELDS.includes(key) && FIELDS.includes(fold(key)));
}

/** The payload as JavaScript reads it, or nothing when it does not. */
const parsed = (result) => {
  try {
    return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(payloadOf(result)));
  } catch {
    return undefined;
  }
};
const STATE = ["account", "seq", "prev", "kind", "signer", "root", "recovery", "recovery_box", "recovery_wrap", "epoch", "devices"];
const DEVICE = ["id", "name", "sign", "box", "wrap"];
const VAULT = ["account", "revision", "head", "epoch", "signer", "data"];
const lacksOne = (value, names) => !value || typeof value !== "object" || names.some((name) => value[name] === undefined || value[name] === null);
/** Whether a field every statement of this kind has is null or not there. */
function lacks(result) {
  const value = parsed(result);
  if (value === undefined) return false;
  if (result.vault) return lacksOne(value, VAULT);
  return lacksOne(value, STATE) || (Array.isArray(value.devices) && value.devices.some((device) => lacksOne(device, DEVICE)));
}
/** Whether some object in the payload's text holds a key twice. */
function repeats(result) {
  let found = false;
  const seen = [];
  const body = payloadOf(result).toString("utf8");
  // Walk the text: an object opens a set of keys, and a string followed by a
  // colon is a key of the object it is in.
  for (let at = 0; at < body.length; at++) {
    const c = body[at];
    if (c === "{") seen.push(new Set());
    else if (c === "[") seen.push(null);
    else if (c === "}" || c === "]") seen.pop();
    else if (c === '"') {
      let end = at + 1;
      while (end < body.length && body[end] !== '"') end += body[end] === backslash ? 2 : 1;
      let key;
      try {
        key = JSON.parse(body.slice(at, end + 1));
      } catch {
        return false;
      }
      at = end;
      if (/^\s*:/.test(body.slice(end + 1, end + 8)) && seen.at(-1)) {
        if (seen.at(-1).has(key)) found = true;
        seen.at(-1).add(key);
      }
    }
  }
  return found;
}
const LIMIT = { event: 32 * 1024, vault: 8 * 1024 * 1024 };
const tooLong = (result) => (result.events ?? []).some((event) => event.payload.length > LIMIT.event) || (result.vault?.payload.length ?? 0) > LIMIT.vault;

// Known differences, each decided and pinned by a case in
// testdata/protocol/histories.json. `when` sees one result whose verdicts
// differ, with Rust refusing, and says whether this entry accounts for it.
const explained = [
  {
    name: "a field's name in another case: Go takes it for the field; JavaScript does not, and Rust refuses the payload",
    when: (result) => folded(result) && result.rust.rule === "payload",
  },
  {
    name: "a field given twice: JavaScript and Go take the last, or for Go the one before a null; Rust refuses the payload",
    when: (result) => repeats(result) && result.rust.rule === "payload",
  },
  {
    name: "a field that is null or left out: Go reads it as empty or zero; JavaScript and Rust need it",
    when: (result) => accepts(result.go) && !accepts(result.js) && result.rust.rule === "payload" && lacks(result),
  },
  {
    name: "a count not written as a whole number (1.0, 1e0, 10e-1): JavaScript reads the number; Go and Rust refuse",
    when: ({ js, go, rust, name }) => accepts(js) && !accepts(go) && rust.rule === "payload" && / written /.test(name),
  },
  {
    name: "a count of -0: JavaScript and Go read zero; Rust refuses",
    when: ({ js, go, rust, name }) => accepts(js) && accepts(go) && rust.rule === "payload" && / written -0$/.test(name),
  },
  {
    name: "a count past 2^53: Go counts it; JavaScript cannot, and Rust refuses what JavaScript would misread",
    when: ({ js, go, rust, name }) => accepts(go) && !accepts(js) && rust.rule === "payload" && / written 90071992547409(92|93)$/.test(name),
  },
  {
    name: "a wrapper, a recovery box or a vault's ciphertext that only has the right length: Go does not decode it; JavaScript and Rust do",
    when: ({ js, go, rust }) => accepts(go) && !accepts(js) && ["wrapper", "data"].includes(rust.rule),
  },
  {
    name: "an incoming-share key that is false, 0 or another falsy value: JavaScript takes it for none; Go and Rust refuse",
    when: ({ js, go, rust, name }) => accepts(js) && !accepts(go) && rust.rule === "payload" && /inbox (added )?as (false|0|""|\[\])/.test(name),
  },
  {
    name: "a byte-order mark before the payload: the viewer drops it; Go, Rust and the hosted service refuse",
    when: ({ js, go, rust, name }) => accepts(js) && !accepts(go) && rust.rule === "payload" && /a byte-order mark first$/.test(name),
  },
  {
    name: "bytes that are not UTF-8: Go reads U+FFFD; the viewer and Rust refuse",
    when: ({ js, go, rust, name }) => accepts(go) && !accepts(js) && rust.rule === "payload" && /not UTF-8|overlong|surrogate encoded/.test(name),
  },
  {
    name: "half a surrogate pair as an escape: JavaScript and Go read it; Rust refuses the payload",
    when: ({ js, go, rust, name }) => accepts(js) && accepts(go) && rust.rule === "payload" && /surrogate pair/.test(name),
  },
  {
    name: "nesting past 127 levels: Rust refuses the payload",
    when: ({ js, go, rust, name }) => accepts(js) && accepts(go) && rust.rule === "payload" && /nested \d+ deep/.test(name),
  },
  {
    name: "a number too large for a double in a field no verifier reads: Rust refuses the payload",
    when: ({ js, go, rust, name }) => accepts(js) && accepts(go) && rust.rule === "payload" && /too large for a double/.test(name),
  },
  {
    name: "a statement longer than the hosted service stores: Rust keeps the service's limit; Go and the viewer have none",
    when: (result) => accepts(result.js) && accepts(result.go) && result.rust.rule === "signed" && tooLong(result),
  },
];

// Each implementation writes the state's fields in its own order.
const canonical = (value) =>
  value && typeof value === "object" && !Array.isArray(value)
    ? Object.fromEntries(Object.keys(value).sort().map((key) => [key, canonical(value[key])]))
    : Array.isArray(value) ? value.map(canonical) : value;
const same = (a, b) => JSON.stringify(canonical(a)) === JSON.stringify(canonical(b));
const counts = new Map(explained.map((entry) => [entry.name, 0]));
const unexplained = [];
const serverDiffers = [];
let agreed = 0;
for (const result of results) {
  const { js, go, rust } = result;
  if (result.server && accepts(result.server) !== accepts(rust)) serverDiffers.push(result);
  if (accepts(js) === accepts(go) && accepts(go) === accepts(rust)) {
    // Where all accept, all must have read the same thing.
    const document = (side) => side.plaintext ?? "";
    if (accepts(rust) && !(same(js.state, go.state) && same(go.state, rust.state) && document(js) === document(go) && document(go) === document(rust))) {
      unexplained.push({ result, why: "every implementation accepts and they do not read the same" });
    } else agreed++;
    continue;
  }
  if (accepts(rust)) {
    unexplained.push({ result, why: "Rust accepts what another implementation refuses" });
    continue;
  }
  const entry = explained.find((candidate) => candidate.when(result));
  if (entry) counts.set(entry.name, counts.get(entry.name) + 1);
  else unexplained.push({ result, why: "the verdicts differ" });
}

const verdicts = ({ js, go, rust, server: hosted }) =>
  `js ${js.verdict}, go ${go.verdict}, rust ${rust.verdict}${rust.rule ? ` (${rust.rule})` : ""}${hosted ? `, server ${hosted.verdict}` : ""}`;
console.log(`${results.length} cases: ${agreed} judged alike by JavaScript, Go and Rust`);
for (const [name, count] of counts) if (count) console.log(`  ${String(count).padStart(5)}  ${name}`);
if (list) {
  for (const result of results) {
    if (accepts(result.js) === accepts(result.go) && accepts(result.go) === accepts(result.rust)) continue;
    console.log(`    ${result.name}: ${verdicts(result)}`);
  }
}
if (server) {
  // Reported, not failed: the service is a fourth implementation in another
  // repository, and what it refuses beyond the others is its own limits.
  const stricter = serverDiffers.filter((result) => accepts(result.rust)).length;
  console.log(`hosted service: differs from Rust on ${serverDiffers.length} cases, refusing ${stricter} that Rust accepts`);
  if (list) for (const result of serverDiffers) console.log(`    ${result.name}: ${verdicts(result)} (${result.server.error ?? ""})`);
}
if (unexplained.length && process.env.PROTOCOL_DIFF_GROUPS) {
  const groups = new Map();
  for (const { result, why } of unexplained) {
    const key = `${why}: ${verdicts(result)}`;
    groups.set(key, [...(groups.get(key) ?? []), result.name]);
  }
  for (const [key, names] of groups) console.error(`${names.length}  ${key}\n      ${names.slice(0, +process.env.PROTOCOL_DIFF_GROUPS).join("\n      ")}`);
  process.exit(1);
}
if (unexplained.length) {
  console.error(`\n${unexplained.length} unexplained:`);
  for (const { result, why } of unexplained.slice(0, list ? Infinity : 40)) {
    console.error(`  ${result.name}\n    ${why}: ${verdicts(result)}`);
    for (const side of ["js", "go", "rust"]) if (result[side].error) console.error(`      ${side}: ${result[side].error}`);
  }
  process.exit(1);
}
console.log("no unexplained difference");
