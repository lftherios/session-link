import os from "node:os";
import path from "node:path";

/**
 * Retroactive importer for pi (earendil-works) sessions
 * (~/.pi/agent/sessions/<encoded-cwd>/<ts>_<id>.jsonl) → session/v0.
 *
 * pi already stores sessions as tree-structured JSONL, one entry per line:
 *   { type: "session", id, cwd, timestamp }              — session header
 *   { type: "model_change", provider, modelId, ... }     — active-model marker
 *   { type: "message", message: { role, content, ... } } — the turns
 * where role ∈ user | assistant | toolResult, content is a part array
 * (text / thinking / toolCall), and assistant messages carry their own
 * provider/model/usage. That's close to session/v0 already, so this is a
 * field rename + the same turn-walk the Claude-Code importer does. Like that
 * importer it's reconstructed, not wire capture: no assembled system prompt or
 * per-call request body, so the run is marked fidelity: "reconstructed".
 */

const dataPart = (data) => ({ type: "data", data });

/** pi's session dir for a cwd: "--" + path (leading slash dropped, / → -) + "--". */
export function piProjectDir(cwd) {
  return path.join(
    os.homedir(),
    ".pi",
    "agent",
    "sessions",
    `--${cwd.replace(/^\//, "").replaceAll("/", "-")}--`,
  );
}

/** True if these first parsed entries look like a pi transcript. */
export function looksLikePi(entries) {
  return entries.some((e) => e?.type === "session" || e?.type === "message");
}

function piPart(block) {
  if (!block || typeof block.type !== "string") return dataPart(block);
  switch (block.type) {
    case "text":
      return { type: "text", text: block.text ?? "" };
    case "thinking":
      return { type: "thinking", text: block.thinking ?? "" };
    case "toolCall":
      if (typeof block.id !== "string" || typeof block.name !== "string")
        return dataPart(block);
      return { type: "tool_call", id: block.id, name: block.name, arguments: block.arguments };
    default:
      return block; // open-world pass-through
  }
}

function piContent(content) {
  if (content == null) return [];
  if (typeof content === "string") return [{ type: "text", text: content }];
  if (!Array.isArray(content)) return [dataPart(content)];
  return content.map(piPart);
}

function piUsage(u) {
  if (!u) return undefined;
  const out = {};
  if (u.input != null) out.input_tokens = u.input;
  if (u.output != null) out.output_tokens = u.output;
  if (u.cacheRead != null) out.cache_read_tokens = u.cacheRead;
  return Object.keys(out).length ? out : undefined;
}

/** A tool result is usually a single text part; surface that plainly, else keep the parts. */
function toolResultValue(content) {
  if (Array.isArray(content) && content.length === 1 && content[0]?.type === "text")
    return content[0].text;
  return content;
}

const firstText = (content) => {
  const parts = piContent(content);
  const t = parts.find((p) => p.type === "text" && p.text.trim());
  return t?.text.trim();
};

export function piSessionToRun(lines, fallbackName) {
  let sessionMeta = null;
  const entries = [];
  for (const line of lines) {
    let e;
    try {
      e = JSON.parse(line);
    } catch {
      continue;
    }
    if (e?.type === "session") sessionMeta = e;
    if (e?.type === "message" && e.message) entries.push(e);
  }
  if (!entries.length) return null;

  const first = entries[0];
  const last = entries[entries.length - 1];
  // Name from the opening user turn (pi has no summary line); trim to a title.
  const opener = entries.find((e) => e.message.role === "user");
  const openerText = opener && firstText(opener.message.content);
  let name = fallbackName;
  if (openerText) name = openerText.length > 80 ? `${openerText.slice(0, 77)}…` : openerText;

  const spans = [
    {
      id: "root",
      parent_id: null,
      type: "agent",
      name,
      started_at: sessionMeta?.timestamp ?? first.timestamp,
      ended_at: last.timestamp,
      status: "ok",
    },
  ];

  // Same walk as the Claude-Code importer: user + toolResult messages
  // accumulate as the pending input delta; each assistant message becomes an
  // llm_call whose input is that delta, and its toolCall blocks become
  // tool_call spans closed by the matching toolResult.
  let pending = [];
  let pendingStart = null;
  let prevTs = first.timestamp;
  const openTools = new Map(); // toolCallId -> span
  let seq = 0;

  for (const e of entries) {
    const msg = e.message;
    if (msg.role === "user") {
      pendingStart ??= e.timestamp;
      pending.push({ role: "user", content: piContent(msg.content) });
    } else if (msg.role === "toolResult") {
      const isError = msg.isError === true || msg.is_error === true;
      const toolSpan = openTools.get(msg.toolCallId);
      if (toolSpan) {
        toolSpan.ended_at = e.timestamp;
        toolSpan.output = {
          result: toolResultValue(msg.content),
          ...(isError ? { is_error: true } : {}),
        };
        if (isError) toolSpan.status = "error";
        openTools.delete(msg.toolCallId);
      }
      // The model sees the result on the next call — keep it in the delta.
      pendingStart ??= e.timestamp;
      pending.push({
        role: "tool",
        content: [
          {
            type: "tool_result",
            tool_call_id: msg.toolCallId,
            ...(isError ? { is_error: true } : {}),
            content: piContent(msg.content),
          },
        ],
      });
    } else if (msg.role === "assistant") {
      const span = {
        id: `s${++seq}`,
        parent_id: "root",
        type: "llm_call",
        name: "turn",
        started_at: prevTs,
        ended_at: e.timestamp,
        status: "ok",
        model: { id: String(msg.model ?? "unknown"), provider: String(msg.provider ?? "unknown") },
        input: { messages: pending },
        output: { messages: [{ role: "assistant", content: piContent(msg.content) }] },
      };
      const usage = piUsage(msg.usage);
      if (usage) span.usage = usage;
      spans.push(span);
      pending = [];
      pendingStart = null;
      for (const block of Array.isArray(msg.content) ? msg.content : []) {
        if (block?.type === "toolCall" && typeof block.id === "string") {
          const toolSpan = {
            id: `s${++seq}`,
            parent_id: span.id,
            type: "tool_call",
            name: block.name,
            started_at: e.timestamp,
            status: "ok",
            input: { name: block.name, arguments: block.arguments, tool_call_id: block.id },
          };
          spans.push(toolSpan);
          openTools.set(block.id, toolSpan);
        }
      }
    }
    prevTs = e.timestamp;
  }

  // Raw is sacred: a transcript ending on a user/tool turn keeps those
  // messages as a custom span rather than fabricating a reply-less llm_call.
  if (pending.length) {
    spans.push({
      id: `s${++seq}`,
      parent_id: "root",
      type: "custom",
      name: "trailing message(s) — transcript ends before a reply",
      started_at: pendingStart ?? prevTs,
      ended_at: prevTs,
      input: { messages: pending },
    });
  }

  return {
    schema: "session/v0",
    name,
    created_at: sessionMeta?.timestamp ?? first.timestamp,
    source: { kind: "import", harness: "pi", label: "pi-import@0.1.0", fidelity: "reconstructed" },
    metadata: {
      ...(sessionMeta?.id ? { session_id: sessionMeta.id } : {}),
      ...(sessionMeta?.cwd ? { cwd: sessionMeta.cwd } : {}),
    },
    spans,
  };
}
