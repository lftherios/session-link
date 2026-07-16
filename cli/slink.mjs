#!/usr/bin/env node
/**
 * session.link CLI — ambient local capture, deliberate publish.
 *
 *   dev    record LLM traffic through a local proxy (nothing leaves the machine)
 *   push   publish one captured session: validate → client-side secret scan →
 *          summary → one confirmation → shareable URL (copied to clipboard)
 *   list   show recent captures
 */
import { spawn } from "node:child_process";
import path from "node:path";
import readline from "node:readline/promises";
import { CAPTURE_DIR, listCaptures, readConfig, writeConfig } from "./store.mjs";
import { dev } from "./proxy.mjs";
import { inspectRunFile, resolveTarget, uploadRun } from "./publish.mjs";
// Static (not dynamic import): the CLI bundles to one file for npm/binary
// distribution, and a single output can't carry lazy import() chunks.
import { open } from "./open.mjs";
import { importSession } from "./import-session.mjs";

const tty = process.stderr.isTTY;
const dim = (s) => (tty ? `\x1b[2m${s}\x1b[0m` : s);
const green = (s) => (tty ? `\x1b[32m${s}\x1b[0m` : s);
const red = (s) => (tty ? `\x1b[31m${s}\x1b[0m` : s);

const HELP = `session.link — capture LLM sessions locally, publish the ones worth sharing

usage
  slink dev [--name <name>] [--port <n>] [-- <command...>]
      Start the recording proxy. With a command, wraps it (sets
      ANTHROPIC_BASE_URL / OPENAI_BASE_URL) and finishes the session when it
      exits; without one, runs until Ctrl-C and prints the env to export.
      Captures land in ${CAPTURE_DIR} — nothing leaves the machine.

  slink push [file] [--pick] [--yes] [--server <url>]
      Publish a captured session (default: the most recent). Validates, scans
      for credentials locally before any bytes leave, shows a summary,
      asks once, then prints the shareable URL and copies it.
        --pick     choose from recent captures
        --yes      skip confirmation (required when stdin is not a TTY)
        --server   target (default: $SLINK_SERVER or https://session.link)

  slink list [--limit <n>]
      Recent captures, newest first.

  slink open [--port <n>] [--no-browser]
      Browse captures at 127.0.0.1 in the exact viewer the hosted site
      uses, and publish from the page you're looking at.

  slink import [<file.jsonl>]
      Convert a coding-agent session transcript (default: this project's
      latest) into a local capture, marked fidelity: reconstructed.

  env: SLINK_UPSTREAM_ANTHROPIC / SLINK_UPSTREAM_OPENAI repoint the proxy
      at compatible upstreams (Ollama, OpenRouter, a mock in tests, ...).

  slink login [--key <rk_...>] [--server <url>]
      Sign in via the browser (GitHub) — opens a page, you approve, the
      key lands in ~/.slink. --key skips the browser and stores a pasted
      key instead (CI, or keys minted with scripts/keys.mjs).`;

function die(msg) {
  console.error(red(`error: ${msg}`));
  process.exit(1);
}

// Value-taking flags are per command: a global list makes one command's
// value flag eat another command's positionals (e.g. --session swallowing
// push's file argument).
const VALUE_FLAGS = {
  dev: ["port", "name"],
  push: ["server", "key"],
  list: ["limit"],
  open: ["port"],
  import: ["session"],
  login: ["key", "server"],
};

function parseArgs(argv) {
  const out = { _: [], flags: {} };
  const command = argv.find((a) => !a.startsWith("--"));
  const valueFlags = VALUE_FLAGS[command] ?? [];
  for (let i = 0; i < argv.length; i++) {
    const a = argv[i];
    if (a === "--") {
      out.cmd = argv.slice(i + 1);
      break;
    }
    if (a.startsWith("--")) {
      const key = a.slice(2);
      if (valueFlags.includes(key)) {
        const value = argv[++i];
        if (value === undefined || value.startsWith("--"))
          die(`--${key} requires a value`);
        out.flags[key] = value;
      } else out.flags[key] = true;
    } else {
      out._.push(a);
    }
  }
  return out;
}

/* ---------------------------------------------------------------- helpers */

function fmtBytes(n) {
  if (n < 1024) return `${n} B`;
  if (n < 1024 * 1024) return `${(n / 1024).toFixed(1)} KB`;
  return `${(n / (1024 * 1024)).toFixed(2)} MB`;
}

function timeAgo(iso) {
  const s = Math.max(0, (Date.now() - new Date(iso).getTime()) / 1000);
  if (s < 60) return `${Math.round(s)}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  if (s < 86400) return `${Math.round(s / 3600)}h ago`;
  return `${Math.round(s / 86400)}d ago`;
}

async function confirm(question) {
  const rl = readline.createInterface({ input: process.stdin, output: process.stderr });
  const answer = await rl.question(question);
  rl.close();
  return /^y(es)?$/i.test(answer.trim());
}

async function clipboard(text) {
  const candidates =
    process.platform === "darwin"
      ? [["pbcopy"]]
      : [["wl-copy"], ["xclip", "-selection", "clipboard"]];
  for (const [cmd, ...args] of candidates) {
    try {
      await new Promise((resolve, reject) => {
        const p = spawn(cmd, args, { stdio: ["pipe", "ignore", "ignore"] });
        p.on("error", reject);
        p.on("close", (c) => (c === 0 ? resolve() : reject(new Error(`exit ${c}`))));
        p.stdin.end(text);
      });
      return true;
    } catch {
      /* try the next one */
    }
  }
  return false;
}

function printCaptures(captures) {
  captures.forEach((c, i) => {
    const marker = c.in_progress ? "  (recording)" : "";
    const models = c.models.length ? `  ${c.models.join(", ")}` : "";
    console.error(
      `  ${String(i + 1).padStart(2)}  ${timeAgo(c.created_at).padEnd(8)} ${(c.name ?? path.basename(c.file)).padEnd(36).slice(0, 36)} ${String(c.spans).padStart(3)} spans${models}${marker}`,
    );
  });
}

/* --------------------------------------------------------------- commands */

async function list(args) {
  const limit = Number(args.flags.limit ?? 10);
  const captures = (await listCaptures()).slice(0, limit);
  if (!captures.length) {
    console.error(`nothing captured yet — try: slink dev -- <command>`);
    return;
  }
  printCaptures(captures);
  console.error(dim(`\n  publish: slink push [--pick]`));
}

async function push(args) {
  const { server, apiKey } = await resolveTarget(args.flags);

  let file = args._[0];
  let capture = null;
  if (!file) {
    const captures = await listCaptures();
    if (!captures.length)
      die("nothing captured yet — run `slink dev -- <cmd>` first, or pass a session/v0 .json path");
    if (args.flags.pick) {
      printCaptures(captures);
      const rl = readline.createInterface({ input: process.stdin, output: process.stderr });
      const answer = await rl.question("publish which? [1] ");
      rl.close();
      const n = Number(answer.trim() || "1");
      capture = captures[n - 1];
      if (!capture) die(`no capture #${n}`);
    } else {
      capture = captures[0];
    }
    file = capture.file;
  }

  const { failure, text, data, bytes, models, errors, secrets } =
    await inspectRunFile(file);
  if (failure) die(failure);
  if (errors.length) {
    console.error(red(`✗ ${path.basename(file)} is not a valid session/v0 document:`));
    for (const e of errors) console.error(`    ${e}`);
    process.exit(1);
  }
  if (secrets.length) {
    console.error(red("✗ publish blocked — the session appears to contain credentials:"));
    for (const h of secrets) console.error(`    ${h.pattern}  ${h.preview}`);
    console.error("  redact them (the capture file is local) and push again");
    process.exit(1);
  }
  console.error(
    `${data.name ?? path.basename(file)} · ${data.spans.length} span${data.spans.length === 1 ? "" : "s"}${models.length ? ` · ${models.join(", ")}` : ""} · ${fmtBytes(bytes)}`,
  );
  console.error(`${green("✓")} no credentials detected`);
  if (data.metadata?.in_progress)
    console.error(dim("  still recording — publishing a snapshot"));
  if (bytes > 2 * 1024 * 1024)
    console.error(red(`  exceeds the server's 2MB ingest cap — this will likely be rejected`));

  if (!args.flags.yes) {
    if (!process.stdin.isTTY)
      die("not a TTY — pass --yes to publish from scripts or CI");
    if (!(await confirm(`Publish unlisted to ${server}? [y/N] `))) {
      console.error("aborted — nothing was uploaded");
      return;
    }
  }

  const { ok, status, out } = await uploadRun(text, { server, apiKey });
  if (!ok) {
    console.error(
      red(`rejected — ${out?.error?.code ?? status}: ${out?.error?.message ?? "unknown error"}`),
    );
    if (out?.error?.details) console.error(JSON.stringify(out.error.details, null, 2));
    if (status === 401)
      console.error("  store a key with: slink login  (mint one: node scripts/keys.mjs create <name>)");
    process.exit(1);
  }

  const copied = await clipboard(out.url);
  const prefix = status === 200 ? "already published" : "published";
  console.error(`${green("✓")} ${prefix}${copied ? "  (URL copied)" : ""}`);
  console.log(out.url); // stdout: the one pipeable line
}

function openBrowser(url) {
  const cmd = process.platform === "darwin" ? "open" : "xdg-open";
  try {
    spawn(cmd, [url], { stdio: "ignore", detached: true }).on("error", () => {}).unref();
  } catch {
    /* the printed URL is the fallback */
  }
}

const sleep = (ms) => new Promise((r) => setTimeout(r, ms));

async function login(args) {
  // Paste path — CI, or operator-minted keys.
  if (args.flags.key) {
    const key = args.flags.key;
    if (!key.startsWith("rk_")) die("that doesn't look like an API key (rk_…)");
    const config = await readConfig();
    config.api_key = key;
    if (args.flags.server) config.server = args.flags.server;
    const file = await writeConfig(config);
    console.error(`${green("✓")} saved to ${file}`);
    return;
  }

  // Browser flow: mint a code, open the approve page, poll until the key drops out.
  const { server } = await resolveTarget(args.flags);
  let res;
  try {
    res = await fetch(`${server}/api/auth/cli`, { method: "POST" });
  } catch (e) {
    die(`cannot reach ${server}: ${e?.message ?? e}`);
  }
  const out = await res.json().catch(() => null);
  if (res.status === 503)
    die(out?.error?.message ?? "browser login isn't configured on this server — pass --key rk_…");
  if (!res.ok || !out?.code) die(`login failed — ${out?.error?.message ?? `HTTP ${res.status}`}`);

  console.error(`Opening ${out.url}`);
  if (out.user_code)
    console.error(`  confirm this code in the browser: ${green(out.user_code)}`);
  console.error(dim("  if the browser doesn't open, visit the URL yourself"));
  openBrowser(out.url);

  const deadline = Date.now() + 10 * 60 * 1000;
  let misses = 0;
  while (Date.now() < deadline) {
    await sleep(2000);
    let poll;
    try {
      poll = await fetch(`${server}/api/auth/cli/${out.code}`);
      misses = 0;
    } catch (e) {
      // Keep polling through blips, but don't pretend all is well forever.
      if (++misses === 5) console.error(dim(`  still trying to reach ${server} (${e?.message ?? e})…`));
      continue;
    }
    if (poll.status === 202) continue;
    if (poll.status === 404) die("login code expired — run `slink login` again");
    if (!poll.ok) die(`login failed — HTTP ${poll.status}`);
    const grant = await poll.json().catch(() => null);
    if (!grant?.key)
      die("login failed — unexpected response from the server; run `slink login` again");
    const config = await readConfig();
    const prev = config.server;
    config.api_key = grant.key;
    config.server = server;
    const file = await writeConfig(config);
    console.error(`${green("✓")} logged in as @${grant.login} to ${server} — saved to ${file}`);
    if (prev && prev !== server)
      console.error(dim(`  note: default push target changed from ${prev} to ${server}`));
    return;
  }
  die(
    misses >= 5
      ? `timed out — never reached ${server}; check the URL and run \`slink login\` again`
      : "timed out waiting for browser approval — run `slink login` again",
  );
}

/* ------------------------------------------------------------------ main */

const args = parseArgs(process.argv.slice(2));
const command = args._.shift();

switch (command) {
  case "dev":
    await dev({
      port: args.flags.port ? Number(args.flags.port) : undefined,
      name: args.flags.name,
      cmd: args.cmd,
    });
    break;
  case "push":
    await push(args);
    break;
  case "list":
    await list(args);
    break;
  case "login":
    await login(args);
    break;
  case "open":
    await open(args.flags);
    break;
  case "import":
    await importSession({ ...args.flags, session: args.flags.session ?? args._[0] });
    break;
  case undefined:
  case "help":
  case "--help":
    console.error(HELP);
    break;
  default:
    die(`unknown command "${command}" — try: dev, push, list, open, import, login`);
}
