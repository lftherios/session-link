import os from "node:os";
import path from "node:path";
import { openSqlite } from "./sqlite.mjs";

/**
 * Retroactive importer for Hermes (NousResearch/hermes-agent) sessions →
 * session/v0.
 *
 * Hermes stores everything in one SQLite DB at ~/.hermes/state.db: a `sessions`
 * row (model, system_prompt, cwd, started_at, token rollups, title) and a flat
 * `messages` table — an OpenAI-style chat log with role/content/tool_calls/
 * tool_call_id/reasoning/token_count. So this walks messages the way the
 * OpenAI normalizer does: user/tool messages accumulate as the input delta,
 * each assistant message becomes an llm_call, its tool_calls become tool_call
 * spans closed by the matching role:"tool" rows.
 *
 * Reconstructed fidelity: full messages/tools/reasoning, but not the wire
 * request bodies. node:sqlite is loaded lazily via ./sqlite.mjs.
 */

const dataPart = (data) => ({ type: "data", data });
const isoSec = (sec) => new Date((typeof sec === "number" ? sec : 0) * 1000).toISOString();

export function hermesDbPath() {
  return process.env.HERMES_STATE_DB || path.join(os.homedir(), ".hermes", "state.db");
}

/* ----------------------------------------------------------- pure builder */

function hermesPart(p) {
  if (typeof p === "string") return { type: "text", text: p };
  if (!p || typeof p.type !== "string") {
    if (p && typeof p.text === "string") return { type: "text", text: p.text };
    return dataPart(p);
  }
  if (p.type === "text") return { type: "text", text: p.text ?? "" };
  return p; // open-world pass-through (image_url, etc.)
}

/** Hermes `content` is usually a plain string, but can be a JSON list/dict. */
function hermesContent(raw) {
  if (raw == null) return [];
  let v = raw;
  if (typeof raw === "string") {
    const t = raw.trim();
    if (t.startsWith("[") || t.startsWith("{")) {
      try {
        v = JSON.parse(raw);
      } catch {
        return [{ type: "text", text: raw }];
      }
    } else {
      return [{ type: "text", text: raw }];
    }
  }
  if (typeof v === "string") return [{ type: "text", text: v }];
  if (Array.isArray(v)) return v.map(hermesPart);
  if (v && typeof v === "object") {
    if (typeof v.text === "string") return [{ type: "text", text: v.text }];
    return [dataPart(v)];
  }
  return [{ type: "text", text: String(v) }];
}

const resultValue = (raw) =>
  typeof raw === "string" ? raw : hermesContent(raw);

/** Hermes stores tool_calls as OpenAI-shaped JSON: [{id, function:{name, arguments}}]. */
function parseToolCalls(raw, seq) {
  if (!raw) return [];
  let arr;
  try {
    arr = typeof raw === "string" ? JSON.parse(raw) : raw;
  } catch {
    return [];
  }
  if (!Array.isArray(arr)) return [];
  return arr
    .map((tc, i) => {
      const fn = tc?.function ?? tc ?? {};
      let args = fn.arguments ?? fn.input ?? fn.args;
      if (typeof args === "string") {
        try {
          args = JSON.parse(args);
        } catch {
          /* keep the raw string */
        }
      }
      const name = fn.name ?? tc?.name;
      if (!name) return null;
      return { id: String(tc?.id ?? tc?.tool_call_id ?? `call_${seq}_${i}`), name: String(name), arguments: args };
    })
    .filter(Boolean);
}

function hermesUsage(m) {
  if (m.token_count == null) return undefined;
  // Per-message count only; for an assistant reply it's the response tokens.
  return { output_tokens: m.token_count };
}

/** session: the raw sessions row. messages: raw message rows ordered by id. */
export function buildHermesRun(session, messages) {
  const name = session.title || session.display_name || session.id || "hermes session";
  const startedAt = isoSec(session.started_at);
  const system = session.system_prompt || undefined;
  const provider = session.billing_provider || "unknown";
  const spans = [{ id: "root", parent_id: null, type: "agent", name, started_at: startedAt }];

  let pending = [];
  let seq = 0;
  let prevTs = session.started_at;
  let lastTs = session.started_at;
  const openTools = new Map();

  for (const m of messages) {
    const role = m.role;
    const ts = m.timestamp;
    if (role === "system") {
      prevTs = ts;
      continue; // captured via session.system_prompt
    }
    if (role === "user") {
      pending.push({ role: "user", content: hermesContent(m.content) });
      lastTs = ts;
    } else if (role === "tool") {
      const span = openTools.get(m.tool_call_id);
      const result = resultValue(m.content);
      if (span) {
        span.ended_at = isoSec(ts);
        span.output = { result };
        openTools.delete(m.tool_call_id);
      }
      pending.push({
        role: "tool",
        content: [{ type: "tool_result", tool_call_id: m.tool_call_id, content: hermesContent(m.content) }],
      });
      lastTs = ts;
    } else if (role === "assistant") {
      const toolCalls = parseToolCalls(m.tool_calls, seq);
      const outContent = [];
      const reasoning = m.reasoning || m.reasoning_content;
      if (reasoning) outContent.push({ type: "thinking", text: String(reasoning) });
      for (const c of hermesContent(m.content)) outContent.push(c);
      for (const tc of toolCalls) outContent.push({ type: "tool_call", id: tc.id, name: tc.name, arguments: tc.arguments });

      const span = {
        id: `s${++seq}`,
        parent_id: "root",
        type: "llm_call",
        name: "turn",
        started_at: isoSec(prevTs),
        ended_at: isoSec(ts),
        status: m.finish_reason === "error" ? "error" : "ok",
        model: { id: String(session.model ?? "unknown"), provider: String(provider) },
        input: { ...(system ? { system } : {}), messages: pending },
        output: { messages: [{ role: "assistant", content: outContent }] },
      };
      const usage = hermesUsage(m);
      if (usage) span.usage = usage;
      spans.push(span);
      pending = [];

      for (const tc of toolCalls) {
        const toolSpan = {
          id: `s${++seq}`,
          parent_id: span.id,
          type: "tool_call",
          name: tc.name,
          started_at: isoSec(ts),
          status: "ok",
          input: { name: tc.name, arguments: tc.arguments, tool_call_id: tc.id },
        };
        spans.push(toolSpan);
        openTools.set(tc.id, toolSpan);
      }
      lastTs = ts;
    }
    prevTs = ts;
  }

  spans[0].ended_at = isoSec(session.ended_at ?? lastTs);
  spans[0].status = "ok";
  if (pending.length) {
    spans.push({
      id: `s${++seq}`,
      parent_id: "root",
      type: "custom",
      name: "trailing message(s) — no assistant reply",
      started_at: isoSec(lastTs),
      ended_at: isoSec(lastTs),
      input: { messages: pending },
    });
  }

  return {
    schema: "session/v0",
    name,
    created_at: startedAt,
    source: { kind: "import", harness: "hermes", label: "session-import@0.1.0", fidelity: "reconstructed" },
    metadata: {
      session_id: session.id,
      ...(session.cwd ? { cwd: session.cwd } : {}),
      ...(session.source ? { origin: session.source } : {}),
    },
    spans,
  };
}

/* ------------------------------------------------------------ DB reading */

const MESSAGE_COLS =
  "role, content, tool_call_id, tool_calls, tool_name, timestamp, token_count, finish_reason, reasoning, reasoning_content";

function readSession(db, id) {
  const session = db.prepare("SELECT * FROM sessions WHERE id = ?").get(id);
  if (!session) return null;
  const messages = db.prepare(`SELECT ${MESSAGE_COLS} FROM messages WHERE session_id = ? ORDER BY id`).all(id);
  return buildHermesRun(session, messages);
}

/** Most recent hermes session whose cwd is this directory. */
export async function latestHermes(cwd) {
  let db;
  try {
    db = await openSqlite(hermesDbPath());
  } catch {
    return null;
  }
  try {
    const row = db.prepare("SELECT id, started_at FROM sessions WHERE cwd = ? ORDER BY started_at DESC LIMIT 1").get(cwd);
    if (!row) return null;
    return { recencyMs: (row.started_at ?? 0) * 1000, load: async () => loadHermes(row.id) };
  } finally {
    db.close();
  }
}

/** Build a run from a specific hermes session id. */
export async function loadHermes(id) {
  const db = await openSqlite(hermesDbPath());
  try {
    const run = readSession(db, id);
    if (!run) throw new Error(`hermes session "${id}" not found in ${hermesDbPath()}`);
    return run;
  } finally {
    db.close();
  }
}
