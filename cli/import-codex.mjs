import { readdir, readFile, stat, open } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

/**
 * Retroactive importer for Codex (openai/codex) sessions → session/v0.
 *
 * Codex writes one JSONL "rollout" per session under
 * ~/.codex/sessions/YYYY/MM/DD/rollout-<ts>-<uuid>.jsonl. Each line is
 * { timestamp, type, payload }: a `session_meta` header (cwd, model_provider,
 * base_instructions = system prompt), `turn_context` lines (per-turn model),
 * `token_count` events (usage), and `response_item` lines carrying OpenAI
 * Responses API items — messages (input_text/output_text), function_call,
 * function_call_output, reasoning — grouped by turn_id.
 *
 * Each turn collapses to one llm_call span; its function_calls become
 * tool_call children closed by the matching function_call_output.
 * Reconstructed fidelity.
 */

const dataPart = (data) => ({ type: "data", data });

export function codexSessionsDir() {
  return path.join(process.env.CODEX_HOME || path.join(os.homedir(), ".codex"), "sessions");
}

/* ----------------------------------------------------------- pure builder */

function mapContent(content) {
  if (typeof content === "string") return content ? [{ type: "text", text: content }] : [];
  if (!Array.isArray(content)) return [];
  return content.map((c) => {
    if (!c || typeof c.type !== "string") return dataPart(c);
    if (c.type === "input_text" || c.type === "output_text" || c.type === "text")
      return { type: "text", text: c.text ?? "" };
    if (c.type === "input_image") return { type: "image", url: c.image_url ?? c.url };
    return c; // open-world pass-through
  });
}

function reasoningText(p) {
  const parts = Array.isArray(p.summary) ? p.summary : Array.isArray(p.content) ? p.content : [];
  const text = parts.map((s) => (typeof s === "string" ? s : s?.text ?? "")).join("\n").trim();
  return text || (p.encrypted_content ? "[reasoning]" : "");
}

function codexUsage(u) {
  if (!u) return undefined;
  const out = {};
  if (u.input_tokens != null) out.input_tokens = u.input_tokens;
  if (u.output_tokens != null) out.output_tokens = u.output_tokens;
  if (u.cached_input_tokens != null) out.cache_read_tokens = u.cached_input_tokens;
  if (u.reasoning_output_tokens != null) out.reasoning_tokens = u.reasoning_output_tokens;
  return Object.keys(out).length ? out : undefined;
}

const systemPrompt = (b) => (typeof b === "string" ? b : typeof b?.text === "string" ? b.text : undefined);
const firstText = (content) => mapContent(content).find((c) => c.type === "text" && c.text.trim())?.text.trim();

export function codexRolloutToRun(lines, fallbackName) {
  let meta = null;
  const turnModels = new Map();
  const items = [];
  for (const line of lines) {
    let e;
    try {
      e = JSON.parse(line);
    } catch {
      continue;
    }
    if (e?.type === "session_meta" && e.payload) meta = e.payload;
    else if (e?.type === "turn_context" && e.payload?.turn_id) turnModels.set(e.payload.turn_id, e.payload.model);
    else if (e?.type === "response_item" || e?.type === "event_msg") items.push(e);
  }
  const responseItems = items.filter((i) => i.type === "response_item");
  if (!meta && !responseItems.length) return null;

  const provider = String(meta?.model_provider ?? "openai");
  const system = systemPrompt(meta?.base_instructions);
  const defaultModel = [...turnModels.values()].filter(Boolean).pop();
  const createdAt = meta?.timestamp ?? items[0]?.timestamp ?? new Date(0).toISOString();

  // Name from the first *real* user prompt. Codex also injects context as
  // role:"user" items (<recommended_plugins>, sandbox notes, …), so the
  // response_item user messages don't distinguish it — but a `user_message`
  // event carries the actual typed prompt verbatim. Fall back to the first
  // user response_item if no such event exists.
  const userEvent = items.find((i) => i.type === "event_msg" && i.payload?.type === "user_message");
  const firstUser = responseItems.find((i) => i.payload?.type === "message" && i.payload.role === "user");
  const openerText = userEvent?.payload?.message || (firstUser && firstText(firstUser.payload.content));
  let name = fallbackName;
  if (openerText) name = openerText.length > 80 ? `${openerText.slice(0, 77)}…` : openerText;

  const spans = [{ id: "root", parent_id: null, type: "agent", name, started_at: createdAt }];

  let pending = [];
  let reasoningBuf = [];
  let toolBuf = []; // { call_id, name, args, startedTs, endedTs, output }
  let lastUsage = null;
  let turnStartTs = createdAt;
  let lastTs = createdAt;
  let seq = 0;

  const flush = (assistantContent, model, endedTs) => {
    seq += 1;
    const out = [];
    for (const r of reasoningBuf) if (r) out.push({ type: "thinking", text: r });
    for (const c of assistantContent) out.push(c);
    for (const tb of toolBuf) out.push({ type: "tool_call", id: tb.call_id, name: tb.name, arguments: tb.args });
    const span = {
      id: `s${seq}`,
      parent_id: "root",
      type: "llm_call",
      name: "turn",
      started_at: turnStartTs,
      ended_at: endedTs,
      status: "ok",
      model: { id: String(model ?? defaultModel ?? "unknown"), provider },
      input: { ...(system ? { system } : {}), messages: pending },
      output: { messages: [{ role: "assistant", content: out }] },
    };
    const usage = codexUsage(lastUsage);
    if (usage) span.usage = usage;
    spans.push(span);
    for (const tb of toolBuf) {
      seq += 1;
      spans.push({
        id: `s${seq}`,
        parent_id: span.id,
        type: "tool_call",
        name: tb.name,
        started_at: tb.startedTs ?? endedTs,
        ended_at: tb.endedTs ?? endedTs,
        status: "ok",
        input: { name: tb.name, arguments: tb.args, tool_call_id: tb.call_id },
        output: { result: tb.output ?? null },
      });
    }
    pending = [];
    reasoningBuf = [];
    toolBuf = [];
    lastUsage = null;
    turnStartTs = endedTs;
  };

  for (const item of items) {
    const ts = item.timestamp ?? lastTs;
    const p = item.payload;
    if (item.type === "event_msg") {
      if (p?.type === "token_count") lastUsage = p.info?.last_token_usage ?? p.info?.total_token_usage ?? lastUsage;
      continue;
    }
    const t = p?.type;
    if (t === "message") {
      if (p.role === "assistant") {
        const model = turnModels.get(p.internal_chat_message_metadata_passthrough?.turn_id);
        flush(mapContent(p.content), model, ts);
      } else {
        if (!pending.length && !toolBuf.length && !reasoningBuf.length) turnStartTs = ts;
        pending.push({ role: p.role ?? "user", content: mapContent(p.content) });
      }
    } else if (t === "function_call") {
      let args = p.arguments;
      if (typeof args === "string") {
        try {
          args = JSON.parse(args);
        } catch {
          /* keep raw */
        }
      }
      toolBuf.push({ call_id: String(p.call_id ?? p.id), name: p.name || "tool", args, startedTs: ts });
    } else if (t === "function_call_output") {
      const tb = toolBuf.find((x) => x.call_id === String(p.call_id));
      if (tb) {
        tb.output = p.output;
        tb.endedTs = ts;
      }
    } else if (t === "reasoning") {
      reasoningBuf.push(reasoningText(p));
    }
    lastTs = ts;
  }

  // An assistant-side buffer with no closing message still becomes a turn;
  // otherwise leftover user input survives as a custom span.
  if (toolBuf.length || reasoningBuf.length) {
    flush([], defaultModel, lastTs);
  } else if (pending.length) {
    seq += 1;
    spans.push({
      id: `s${seq}`,
      parent_id: "root",
      type: "custom",
      name: "trailing message(s) — no assistant reply",
      started_at: turnStartTs,
      ended_at: lastTs,
      input: { messages: pending },
    });
  }

  spans[0].ended_at = lastTs;
  spans[0].status = "ok";

  return {
    schema: "session/v0",
    name,
    created_at: createdAt,
    source: { kind: "import", harness: "codex", label: "session-import@0.1.0", fidelity: "reconstructed" },
    metadata: {
      ...(meta?.session_id ? { session_id: meta.session_id } : {}),
      ...(meta?.cwd ? { cwd: meta.cwd } : {}),
      ...(meta?.cli_version ? { harness_version: meta.cli_version } : {}),
    },
    spans,
  };
}

export function looksLikeCodex(entries) {
  return entries.some((e) => e?.type === "session_meta" || (e?.type === "response_item" && e?.payload));
}

/* ------------------------------------------------------------ locating */

async function readFirstLine(file) {
  // The session_meta line embeds Codex's whole system prompt, so it can be
  // tens of KB — read (as raw bytes, to avoid splitting a multibyte char)
  // until the first newline rather than assuming a fixed window.
  const fh = await open(file, "r");
  try {
    const chunks = [];
    const buf = Buffer.alloc(64 * 1024);
    let pos = 0;
    let total = 0;
    for (;;) {
      const { bytesRead } = await fh.read(buf, 0, buf.length, pos);
      if (!bytesRead) break;
      const slice = Buffer.from(buf.subarray(0, bytesRead));
      const nl = slice.indexOf(0x0a);
      if (nl !== -1) {
        chunks.push(slice.subarray(0, nl));
        return Buffer.concat(chunks).toString("utf8");
      }
      chunks.push(slice);
      pos += bytesRead;
      total += bytesRead;
      if (total > 4 * 1024 * 1024) break; // a single line this long isn't a header we care about
    }
    return Buffer.concat(chunks).toString("utf8");
  } finally {
    await fh.close();
  }
}

async function parseRollout(file) {
  const text = await readFile(file, "utf8");
  return codexRolloutToRun(text.split("\n").filter(Boolean), path.basename(file, ".jsonl"));
}

/** Most recent codex rollout whose session_meta.cwd is this directory. */
export async function latestCodex(cwd) {
  const dir = codexSessionsDir();
  let names;
  try {
    names = await readdir(dir, { recursive: true });
  } catch {
    return null;
  }
  const rollouts = [];
  for (const n of names) {
    if (!n.endsWith(".jsonl")) continue;
    const full = path.join(dir, n);
    try {
      rollouts.push({ full, mtimeMs: (await stat(full)).mtimeMs });
    } catch {
      /* skip */
    }
  }
  rollouts.sort((a, b) => b.mtimeMs - a.mtimeMs);
  for (const r of rollouts.slice(0, 500)) {
    let head;
    try {
      head = JSON.parse(await readFirstLine(r.full));
    } catch {
      continue;
    }
    if (head?.type === "session_meta" && head.payload?.cwd === cwd)
      return { recencyMs: r.mtimeMs, load: () => parseRollout(r.full) };
  }
  return null;
}

/** Import a specific rollout file. */
export async function loadCodexFile(file) {
  const run = await parseRollout(file);
  if (!run) throw new Error(`no importable items in ${file}`);
  return run;
}
