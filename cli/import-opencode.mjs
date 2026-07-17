import os from "node:os";
import path from "node:path";
import { openSqlite } from "./sqlite.mjs";

/**
 * Retroactive importer for opencode (sst/opencode) sessions → session/v0.
 *
 * opencode (>= the SQL migration, v1.18.x) stores everything in one SQLite DB
 * at $XDG_DATA_HOME/opencode/opencode.db: a `session` row (title, directory,
 * model, token rollups), `message` rows (one per user/assistant turn, with a
 * JSON `data` blob), and `part` rows (the turn's content — text, reasoning,
 * and `tool` parts that carry both the call and its result). Each assistant
 * message becomes an llm_call span; its tool parts become tool_call children.
 *
 * Reconstructed fidelity: rich messages/tools/usage, but not the wire request
 * bodies. node:sqlite is loaded lazily so the rest of the CLI still runs on
 * Node versions without it.
 */

const dataPart = (data) => ({ type: "data", data });
const iso = (ms) => new Date(typeof ms === "number" ? ms : 0).toISOString();
const safeParse = (s) => {
  if (s && typeof s === "object") return s;
  try {
    return JSON.parse(s);
  } catch {
    return null;
  }
};

export function opencodeDbPath() {
  const dataHome = process.env.XDG_DATA_HOME || path.join(os.homedir(), ".local", "share");
  return path.join(dataHome, "opencode", "opencode.db");
}

/* ----------------------------------------------------------- pure builder */

function userContent(parts) {
  const out = [];
  for (const p of parts) {
    if (p?.type === "text") out.push({ type: "text", text: p.text ?? "" });
    else if (p?.type) out.push(dataPart(p)); // file, etc. — keep, don't drop
  }
  return out;
}

/** Assistant content parts → session/v0 parts (text, reasoning→thinking, tool→tool_call). */
function assistantContent(parts) {
  const out = [];
  for (const p of parts) {
    if (p?.type === "text") out.push({ type: "text", text: p.text ?? "" });
    else if (p?.type === "reasoning") out.push({ type: "thinking", text: p.text ?? "" });
    else if (p?.type === "tool" && typeof p.callID === "string")
      out.push({ type: "tool_call", id: p.callID, name: p.tool, arguments: p.state?.input });
    // step-start / step-finish / patch are structural, not content — skip.
  }
  return out;
}

function ocUsage(tokens) {
  if (!tokens) return undefined;
  const out = {};
  if (tokens.input != null) out.input_tokens = tokens.input;
  if (tokens.output != null) out.output_tokens = tokens.output;
  if (tokens.reasoning != null) out.reasoning_tokens = tokens.reasoning;
  if (tokens.cache?.read != null) out.cache_read_tokens = tokens.cache.read;
  return Object.keys(out).length ? out : undefined;
}

/**
 * session: the raw `session` row. messages: [{ id, time_created, data, parts }]
 * ordered by time, where data is the parsed message JSON and parts the parsed
 * part JSON blobs for that message (also time-ordered).
 */
export function buildOpencodeRun(session, messages) {
  const model = safeParse(session.model) ?? {};
  const name = session.title || session.id || "opencode session";
  const startedAt = iso(session.time_created);
  const spans = [{ id: "root", parent_id: null, type: "agent", name, started_at: startedAt }];

  let pending = [];
  let seq = 0;
  let lastTs = session.time_created;

  for (const m of messages) {
    const d = m.data ?? {};
    if (d.role === "user") {
      pending.push({ role: "user", content: userContent(m.parts ?? []) });
      lastTs = m.time_created;
    } else if (d.role === "assistant") {
      const created = d.time?.created ?? m.time_created;
      const completed = d.time?.completed ?? created;
      const errored = Boolean(d.error) || d.finish === "error";
      const span = {
        id: `s${++seq}`,
        parent_id: "root",
        type: "llm_call",
        name: "turn",
        started_at: iso(created),
        ended_at: iso(completed),
        status: errored ? "error" : "ok",
        model: {
          id: String(d.modelID ?? model.id ?? "unknown"),
          provider: String(d.providerID ?? model.providerID ?? "unknown"),
        },
        input: { messages: pending },
        output: { messages: [{ role: "assistant", content: assistantContent(m.parts ?? []) }] },
      };
      const usage = ocUsage(d.tokens);
      if (usage) span.usage = usage;
      if (errored) span.error = { message: String(d.error?.message ?? d.error ?? "error") };
      spans.push(span);
      pending = [];

      for (const p of m.parts ?? []) {
        if (p?.type === "tool" && typeof p.callID === "string") {
          const st = p.state ?? {};
          const isErr = st.status === "error";
          spans.push({
            id: `s${++seq}`,
            parent_id: span.id,
            type: "tool_call",
            name: p.tool || "tool",
            started_at: iso(st.time?.start ?? created),
            ended_at: iso(st.time?.end ?? completed),
            status: isErr ? "error" : "ok",
            input: { name: p.tool, arguments: st.input, tool_call_id: p.callID },
            output: { result: st.output ?? st.error ?? st.metadata?.output ?? null, ...(isErr ? { is_error: true } : {}) },
          });
        }
      }
      lastTs = completed;
    }
    // other roles (summaries, etc.) are ignored
  }

  spans[0].ended_at = iso(lastTs);
  spans[0].status = "ok";
  if (pending.length) {
    spans.push({
      id: `s${++seq}`,
      parent_id: "root",
      type: "custom",
      name: "trailing message(s) — no assistant reply",
      started_at: iso(lastTs),
      ended_at: iso(lastTs),
      input: { messages: pending },
    });
  }

  return {
    schema: "session/v0",
    name,
    created_at: startedAt,
    source: { kind: "import", harness: "opencode", label: "session-import@0.1.0", fidelity: "reconstructed" },
    metadata: {
      session_id: session.id,
      ...(session.directory ? { cwd: session.directory } : {}),
      ...(session.version ? { harness_version: session.version } : {}),
    },
    spans,
  };
}

/* ------------------------------------------------------------ DB reading */

function readSession(db, id) {
  const session = db.prepare("SELECT * FROM session WHERE id = ?").get(id);
  if (!session) return null;
  const msgRows = db.prepare("SELECT id, time_created, data FROM message WHERE session_id = ? ORDER BY time_created, id").all(id);
  const partRows = db.prepare("SELECT message_id, time_created, data FROM part WHERE session_id = ? ORDER BY time_created, id").all(id);
  const partsByMsg = new Map();
  for (const p of partRows) {
    const arr = partsByMsg.get(p.message_id) ?? [];
    arr.push(safeParse(p.data) ?? {});
    partsByMsg.set(p.message_id, arr);
  }
  const messages = msgRows.map((m) => ({
    id: m.id,
    time_created: m.time_created,
    data: safeParse(m.data) ?? {},
    parts: partsByMsg.get(m.id) ?? [],
  }));
  return buildOpencodeRun(session, messages);
}

/** Most recent opencode session whose directory is this cwd. */
export async function latestOpencode(cwd) {
  let db;
  try {
    db = await openSqlite(opencodeDbPath());
  } catch {
    return null; // no DB / no node:sqlite — this harness just isn't available
  }
  try {
    const row = db.prepare("SELECT id, time_created FROM session WHERE directory = ? ORDER BY time_created DESC LIMIT 1").get(cwd);
    if (!row) return null;
    return {
      recencyMs: row.time_created,
      load: async () => loadOpencode(row.id),
    };
  } finally {
    db.close();
  }
}

/** Build a run from a specific opencode session id. */
export async function loadOpencode(id) {
  const db = await openSqlite(opencodeDbPath());
  try {
    const run = readSession(db, id);
    if (!run) throw new Error(`opencode session "${id}" not found in ${opencodeDbPath()}`);
    return run;
  } finally {
    db.close();
  }
}
