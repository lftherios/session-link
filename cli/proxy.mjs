import http from "node:http";
import { spawn } from "node:child_process";
import { CAPTURE_DIR, newCapturePath, writeRun } from "./store.mjs";
import {
  assembleAnthropicSse,
  assembleOpenaiChatSse,
  assembleResponsesSse,
  buildLlmSpan,
  parseSseText,
} from "./normalize.mjs";
import { SessionRouter, sessionKeyFrom, labelFrom } from "./tap.mjs";
import { autoPrune } from "./prune.mjs";

/**
 * The recording proxy behind `slink dev`. Forwards /anthropic/* and
 * /openai/* to the real APIs, streams responses through untouched, and
 * tees recordable calls into one session/v0 capture per session. Capture is
 * local-only by design — nothing leaves the machine until `slink push`.
 */

const CLI_LABEL = "slink-cli@0.1.0";

const UPSTREAMS = {
  anthropic: process.env.SLINK_UPSTREAM_ANTHROPIC ?? "https://api.anthropic.com",
  openai: process.env.SLINK_UPSTREAM_OPENAI ?? "https://api.openai.com",
};

// Hop-by-hop / transport headers: never forwarded upstream. accept-encoding
// is dropped because fetch decompresses — echoing content-encoding back
// with a decompressed body would corrupt the client's read.
const REQ_DROP = new Set([
  "host",
  "connection",
  "content-length",
  "transfer-encoding",
  "accept-encoding",
  "keep-alive",
  "upgrade",
  "te",
  "trailer",
  "proxy-authorization",
  // slink-internal routing hints — used to key sessions, never sent upstream.
  "x-slink-session",
  "x-slink-label",
]);
const RES_DROP = new Set([
  "content-encoding",
  "content-length",
  "transfer-encoding",
  "connection",
  "keep-alive",
]);

// Stop teeing into memory past this point (the client still gets the full
// stream); the span is marked fidelity: partial.
const CAPTURE_CAP = 32 * 1024 * 1024;

const tty = process.stderr.isTTY;
const dim = (s) => (tty ? `\x1b[2m${s}\x1b[0m` : s);
const green = (s) => (tty ? `\x1b[32m${s}\x1b[0m` : s);
const red = (s) => (tty ? `\x1b[31m${s}\x1b[0m` : s);
const log = (s) => process.stderr.write(`${s}\n`);

function endpointKind(provider, subpath) {
  const p = subpath.split("?")[0].replace(/\/$/, "");
  if (provider === "anthropic" && p === "/v1/messages") return "anthropic.messages";
  if (provider === "openai" && p.endsWith("/chat/completions")) return "openai.chat";
  if (provider === "openai" && p.endsWith("/responses")) return "openai.responses";
  return null; // pass through unrecorded (embeddings, models, batches, ...)
}

function newSession(name, command) {
  const now = new Date();
  return {
    file: newCapturePath(now),
    seq: 0,
    writes: Promise.resolve(),
    run: {
      schema: "session/v0",
      name,
      created_at: now.toISOString(),
      source: { kind: "proxy", label: CLI_LABEL, fidelity: "exact" },
      metadata: { ...(command ? { command } : {}), cwd: process.cwd(), in_progress: true },
      spans: [
        { id: "root", parent_id: null, type: "agent", name, started_at: now.toISOString() },
      ],
    },
  };
}

/**
 * All capture writes go through one per-session chain: concurrent calls,
 * and the finalize on exit, must land in order — the README's own
 * `slink dev -- <fast command>` used to crash when finish()'s write raced
 * the last call's still-in-flight flush.
 */
function flush(session) {
  session.writes = session.writes
    .catch(() => {}) // an earlier failed write must not poison the chain
    .then(() => writeRun(session.file, session.run))
    .catch((e) => {
      // A failing capture disk must not be silent: warn once, keep proxying.
      if (!session.writeFailed) {
        session.writeFailed = true;
        log(red(`capture write failed: ${e?.message ?? e} — recording may be incomplete`));
      }
      throw e;
    });
  return session.writes;
}

/** Flushed to disk after every call — a crash never loses captured spans. */
async function record(session, span) {
  session.seq += 1;
  session.run.spans.push(span);
  if (span.ended_at) session.run.spans[0].ended_at = span.ended_at;
  await flush(session);
}

function logSpan(span, ms) {
  const dur = ms < 1000 ? `${Math.round(ms)}ms` : `${(ms / 1000).toFixed(1)}s`;
  const model = `${span.model.provider}/${span.model.id}`;
  if (span.status === "error") {
    log(red(`✗ ${span.id} llm_call ${model} · ${span.error?.message ?? "error"}`));
    return;
  }
  const tok = span.usage
    ? ` · ${span.usage.input_tokens ?? "?"}→${span.usage.output_tokens ?? "?"} tok`
    : "";
  const stream = span.metadata?.stream ? " · stream" : "";
  log(`${green("●")} ${span.id} llm_call ${model} · ${dur}${tok}${stream}`);
}

async function recordCall(session, kind, reqBuf, resText, isSse, started, ended, httpStatus, transportError, overflow = false) {
  let request = null;
  try {
    request = JSON.parse(reqBuf.toString("utf8"));
  } catch {
    /* leave null; raw.request stays null */
  }
  let response = null;
  let assembled = true;
  if (resText != null) {
    if (isSse) {
      const events = parseSseText(resText);
      response =
        kind === "anthropic.messages"
          ? assembleAnthropicSse(events)
          : kind === "openai.chat"
            ? assembleOpenaiChatSse(events)
            : assembleResponsesSse(events);
      if (!response) {
        assembled = false;
        response = { unassembled_sse: resText.slice(0, 100_000) };
      }
    } else {
      try {
        response = JSON.parse(resText);
      } catch {
        assembled = false;
        response = { unparsed_body: resText.slice(0, 100_000) };
      }
    }
  }
  const span = buildLlmSpan(kind, {
    id: `s${session.seq + 1}`,
    parent_id: "root",
    request,
    response,
    started_at: started.toISOString(),
    ended_at: ended.toISOString(),
    httpStatus,
    streamed: isSse,
    captureGap: overflow || !assembled,
    transportError,
  });
  await record(session, span);
  logSpan(span, ended - started);
}

function safeJson(buf) {
  try {
    return JSON.parse(buf.toString("utf8"));
  } catch {
    return null;
  }
}

/** Name a fresh session from its first call: the user's opening text, else the model. */
function deriveName(request) {
  if (!request) return undefined;
  const messages = Array.isArray(request.messages) ? request.messages : [];
  const firstUser = messages.find((m) => m?.role === "user");
  let text = "";
  const content = firstUser?.content;
  if (typeof content === "string") text = content;
  else if (Array.isArray(content))
    text = content.map((p) => (typeof p?.text === "string" ? p.text : "")).join(" ");
  text = text.trim();
  if (text) return text.length > 60 ? `${text.slice(0, 57)}…` : text;
  return request.model ? String(request.model) : undefined;
}

/**
 * The shared request handler. `sink` decides which session a recordable call
 * belongs to: one fixed session under `dev`, or header/idle-routed under the
 * always-on tap (`serve`).
 */
async function handle(sink, req, res) {
  if (req.url === "/" && req.method === "GET") {
    res.writeHead(200, { "content-type": "application/json" });
    res.end(JSON.stringify({ service: "session.link", ...sink.status() }));
    return;
  }
  const m = (req.url ?? "").match(/^\/(anthropic|openai)(\/.*)$/);
  if (!m) {
    res.writeHead(404, { "content-type": "application/json" });
    res.end(
      JSON.stringify({
        error: { code: "unknown_route", message: "expected /anthropic/* or /openai/*" },
      }),
    );
    return;
  }
  const [, provider, subpath] = m;
  const kind = req.method === "POST" ? endpointKind(provider, subpath) : null;

  const reqChunks = [];
  for await (const c of req) reqChunks.push(c);
  const reqBuf = Buffer.concat(reqChunks);

  // Resolve the session for a recordable call up front — the tap keys it by
  // the x-slink-session header (or idle-segmented ambient), dev ignores it.
  let session = null;
  if (kind) {
    const request = safeJson(reqBuf);
    session = sink.resolve(sessionKeyFrom(req.headers), labelFrom(req.headers) ?? deriveName(request));
  }

  const headers = {};
  for (const [k, v] of Object.entries(req.headers)) {
    if (!REQ_DROP.has(k.toLowerCase()) && v !== undefined) headers[k] = v;
  }

  const started = new Date();
  let upstream;
  try {
    upstream = await fetch(UPSTREAMS[provider] + subpath, {
      method: req.method,
      headers,
      body: reqBuf.length ? reqBuf : undefined,
    });
  } catch (e) {
    const message = `upstream unreachable: ${e?.message ?? e}`;
    res.writeHead(502, { "content-type": "application/json" });
    res.end(JSON.stringify({ error: { code: "upstream_unreachable", message } }));
    if (session) await recordCall(session, kind, reqBuf, null, false, started, new Date(), 502, message);
    return;
  }

  const resHeaders = {};
  for (const [k, v] of upstream.headers) {
    if (!RES_DROP.has(k.toLowerCase())) resHeaders[k] = v;
  }
  res.writeHead(upstream.status, resHeaders);

  const isSse = (upstream.headers.get("content-type") ?? "").includes("text/event-stream");
  const resChunks = [];
  let captured = 0;
  let overflow = false;
  if (upstream.body) {
    for await (const chunk of upstream.body) {
      res.write(chunk);
      if (captured < CAPTURE_CAP) {
        resChunks.push(Buffer.from(chunk));
        captured += chunk.length;
      } else {
        overflow = true;
      }
    }
  }
  res.end();

  if (!session) return;
  const resText = Buffer.concat(resChunks).toString("utf8");
  await recordCall(session, kind, reqBuf, resText, isSse, started, new Date(), upstream.status, null, overflow);
}

export async function dev({ port, name, cmd }) {
  const runName = name ?? (cmd?.length ? cmd.join(" ") : "proxy session");
  const session = newSession(runName, cmd?.length ? cmd.join(" ") : undefined);
  // dev is one session for the whole invocation — the sink ignores the key.
  const sink = {
    resolve: () => session,
    status: () => ({ recording: session.file, calls: session.seq }),
  };

  // finish() must wait these out: the client gets its response before the
  // capture is flushed, so a wrapped command can exit while recording of
  // its last call is still in flight.
  const inflight = new Set();
  const server = http.createServer((req, res) => {
    const p = handle(sink, req, res).catch((e) => {
      // Only answer if the response is still answerable — a failure AFTER
      // the response ended is a capture problem, already warned by flush().
      if (res.writableEnded) return;
      if (!res.headersSent) res.writeHead(500, { "content-type": "application/json" });
      res.end(JSON.stringify({ error: { code: "proxy_error", message: String(e?.message ?? e) } }));
    });
    inflight.add(p);
    p.finally(() => inflight.delete(p));
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    // Wrapper mode picks an ephemeral port; daemon mode a stable one.
    server.listen(port ?? (cmd?.length ? 0 : 4141), "127.0.0.1", resolve);
  });
  const p = server.address().port;
  const env = {
    ANTHROPIC_BASE_URL: `http://127.0.0.1:${p}/anthropic`,
    OPENAI_BASE_URL: `http://127.0.0.1:${p}/openai/v1`,
  };
  log(`session.link proxy · http://127.0.0.1:${p} · recording ${dim(session.file)}`);

  const finish = async () => {
    server.close();
    server.closeAllConnections?.();
    if (inflight.size > 0) log(dim(`waiting for ${inflight.size} in-flight call(s)…`));
    // Bounded: a wedged upstream stream must not hold the exit hostage.
    const drained = await Promise.race([
      Promise.allSettled([...inflight]).then(() => true),
      new Promise((r) => setTimeout(r, 5000, false).unref?.()),
    ]);
    if (!drained) log(red(`gave up on ${inflight.size} in-flight call(s) after 5s — capture may be missing them`));
    if (session.seq === 0) {
      log("no LLM calls captured — nothing saved");
      return;
    }
    const root = session.run.spans[0];
    root.ended_at = new Date().toISOString();
    root.status = "ok";
    delete session.run.metadata.in_progress;
    try {
      await flush(session);
    } catch (e) {
      // One clean line, and never clobber the child's own exit code.
      log(red(`failed to save capture: ${e?.message ?? e}`));
      process.exitCode ||= 1;
      return;
    }
    log(`recorded ${session.seq} call${session.seq === 1 ? "" : "s"} → ${session.file}`);
    log(dim("publish: slink push"));
  };

  if (cmd?.length) {
    // Ctrl-C goes to the child (same process group); we finalize after it exits.
    process.on("SIGINT", () => {});
    const child = spawn(cmd[0], cmd.slice(1), {
      stdio: "inherit",
      env: { ...process.env, ...env },
    });
    const code = await new Promise((resolve) => {
      child.on("error", (e) => {
        log(red(`failed to start ${cmd[0]}: ${e.message}`));
        resolve(1);
      });
      child.on("exit", (c) => resolve(c ?? 0));
    });
    await finish();
    process.exitCode = code;
  } else {
    for (const [k, v] of Object.entries(env)) log(`export ${k}=${v}`);
    log(dim("Ctrl-C to finish"));
    await new Promise((resolve) => process.once("SIGINT", resolve));
    log("");
    await finish();
  }
}

/** Close out a session the tap has segmented: stamp the root, drop the in-progress flag, flush. */
async function finalizeSession(session) {
  if (session.seq === 0) return; // never recorded anything — nothing to save
  const root = session.run.spans[0];
  root.ended_at = new Date().toISOString();
  root.status = "ok";
  delete session.run.metadata.in_progress;
  try {
    await flush(session);
  } catch (e) {
    log(red(`failed to save capture: ${e?.message ?? e}`));
    return;
  }
  log(`${green("●")} session ended · ${session.seq} call${session.seq === 1 ? "" : "s"} → ${dim(session.file)}`);
}

/**
 * The always-on tap: a long-running recorder on a stable port. Every provider
 * call flowing through it is grouped into a session (by the x-slink-session
 * header, or idle-segmented ambient) and written to a local capture — nothing
 * is published. Point your agents' ANTHROPIC_BASE_URL / OPENAI_BASE_URL here
 * and capture becomes a background fact.
 */
export async function serve({ port = 4141, idleMs } = {}) {
  const inflight = new Set();
  const track = (p) => {
    inflight.add(p);
    p.finally(() => inflight.delete(p));
    return p;
  };
  const router = new SessionRouter({
    ...(idleMs ? { idleMs } : {}),
    newSession: (name) => newSession(name ?? "ambient session"),
    onFinalize: (session) => track(finalizeSession(session)),
  });
  const sink = {
    resolve: (key, name) => router.route(key, name),
    status: () => ({ sessions: router.size }),
  };

  const server = http.createServer((req, res) => {
    track(
      handle(sink, req, res).catch((e) => {
        if (res.writableEnded) return;
        if (!res.headersSent) res.writeHead(500, { "content-type": "application/json" });
        res.end(JSON.stringify({ error: { code: "proxy_error", message: String(e?.message ?? e) } }));
      }),
    );
  });
  await new Promise((resolve, reject) => {
    server.once("error", reject);
    server.listen(port, "127.0.0.1", resolve);
  });
  const p = server.address().port;
  const env = {
    ANTHROPIC_BASE_URL: `http://127.0.0.1:${p}/anthropic`,
    OPENAI_BASE_URL: `http://127.0.0.1:${p}/openai/v1`,
  };
  log(`session.link tap · http://127.0.0.1:${p} · capturing to ${dim(CAPTURE_DIR)}`);

  // Rolling buffer: sweep old/empty captures on start so ambient recording
  // doesn't grow ~/.slink without bound (SLINK_RETAIN_DAYS, default 30).
  const pruned = await autoPrune();
  if (pruned) log(dim(`pruned ${pruned} old capture${pruned === 1 ? "" : "s"}`));

  // Finalize quiet sessions without waiting for the next call to arrive.
  const timer = setInterval(() => router.sweep(), Math.min(router.idleMs, 60_000));
  timer.unref?.();

  for (const [k, v] of Object.entries(env)) log(`export ${k}=${v}`);
  log(dim("Ctrl-C to stop"));

  await new Promise((resolve) => process.once("SIGINT", resolve));
  log("");
  clearInterval(timer);
  server.close();
  server.closeAllConnections?.();
  router.finalizeAll(); // rolls every open session through onFinalize
  await Promise.race([
    Promise.allSettled([...inflight]).then(() => true),
    new Promise((r) => setTimeout(r, 5000, false).unref?.()),
  ]);
}
