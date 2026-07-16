import { readdir, readFile, stat } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { newCapturePath, writeRun } from "./store.mjs";
import { anthropicContent, anthropicUsage } from "./normalize.mjs";

/**
 * Retroactive importer: turn a coding-agent session transcript
 * (~/.claude/projects/<encoded-cwd>/<session>.jsonl) into a session/v0 capture
 * — works with zero prior setup, for the session you already had.
 *
 * Reconstructed, not wire capture: the transcript has messages, tool
 * calls/results, models, and usage, but not the assembled system prompt or
 * per-call request bodies — so the run is marked fidelity: "reconstructed"
 * and each llm_call's input holds only the messages new since the previous
 * call. Message content is Anthropic-shaped and reuses the proxy's
 * normalizers; sidechain (subagent) entries are skipped and counted.
 */

const IMPORT_LABEL = "session-import@0.1.0";

function projectDir(cwd) {
  return path.join(
    os.homedir(),
    ".claude",
    "projects",
    cwd.replaceAll("/", "-").replaceAll(".", "-"),
  );
}

async function latestSession(dir) {
  let files;
  try {
    files = await readdir(dir);
  } catch {
    return null;
  }
  let best = null;
  for (const f of files) {
    if (!f.endsWith(".jsonl")) continue;
    const full = path.join(dir, f);
    const s = await stat(full);
    if (!best || s.mtimeMs > best.mtimeMs) best = { full, mtimeMs: s.mtimeMs };
  }
  return best?.full ?? null;
}

export function transcriptToRun(lines, sessionName) {
  const entries = [];
  let skippedSidechain = 0;
  let name = sessionName;
  for (const line of lines) {
    let e;
    try {
      e = JSON.parse(line);
    } catch {
      continue;
    }
    if (e?.type === "summary" && e.summary) name = e.summary;
    if (e?.type !== "user" && e?.type !== "assistant") continue;
    if (e.isSidechain) {
      skippedSidechain++;
      continue;
    }
    if (!e.message) continue;
    entries.push(e);
  }
  if (!entries.length) return null;

  const first = entries[0];
  const spans = [
    {
      id: "root",
      parent_id: null,
      type: "agent",
      name,
      started_at: first.timestamp,
      ended_at: entries[entries.length - 1].timestamp,
      status: "ok",
    },
  ];

  // Walk turns: user/tool messages accumulate as the pending delta; each
  // assistant entry becomes an llm_call whose input is that delta. tool_use
  // blocks become tool_call spans, closed by the matching tool_result.
  let pending = [];
  let pendingStart = null; // timestamp of the first message in the pending delta
  let prevTs = first.timestamp;
  const openTools = new Map(); // tool_use_id -> span
  let seq = 0;

  for (const e of entries) {
    const msg = e.message;
    if (e.type === "user") {
      const content = anthropicContent(msg.content);
      for (const part of content) {
        if (part.type === "tool_result") {
          const toolSpan = openTools.get(part.tool_call_id);
          if (toolSpan) {
            toolSpan.ended_at = e.timestamp;
            toolSpan.output = {
              result: e.toolUseResult ?? part.content,
              ...(part.is_error ? { is_error: true } : {}),
            };
            if (part.is_error) toolSpan.status = "error";
            openTools.delete(part.tool_call_id);
          }
        }
      }
      pendingStart ??= e.timestamp;
      pending.push({ role: "user", content });
    } else {
      const span = {
        id: `s${++seq}`,
        parent_id: "root",
        type: "llm_call",
        name: "turn",
        started_at: prevTs,
        ended_at: e.timestamp,
        status: "ok",
        model: { id: String(msg.model ?? "unknown"), provider: "anthropic" },
        input: { messages: pending },
        output: { messages: [{ role: "assistant", content: anthropicContent(msg.content) }] },
      };
      const usage = anthropicUsage(msg.usage);
      if (usage) span.usage = usage;
      spans.push(span);
      pending = [];
      pendingStart = null;
      for (const block of Array.isArray(msg.content) ? msg.content : []) {
        if (block?.type === "tool_use" && typeof block.id === "string") {
          const toolSpan = {
            id: `s${++seq}`,
            parent_id: span.id,
            type: "tool_call",
            name: block.name,
            started_at: e.timestamp,
            status: "ok",
            input: { name: block.name, arguments: block.input, tool_call_id: block.id },
          };
          spans.push(toolSpan);
          openTools.set(block.id, toolSpan);
        }
      }
    }
    prevTs = e.timestamp;
  }

  // A transcript can end on a user turn (no assistant reply yet). Raw is
  // sacred: keep those messages as a custom span instead of dropping them —
  // fabricating an llm_call with no response would be dishonest.
  if (pending.length) {
    spans.push({
      id: `s${++seq}`,
      parent_id: "root",
      type: "custom",
      name: "trailing user message(s) — transcript ends before a reply",
      started_at: pendingStart ?? prevTs,
      ended_at: prevTs,
      input: { messages: pending },
    });
  }

  return {
    schema: "session/v0",
    name,
    created_at: first.timestamp,
    source: { kind: "import", label: IMPORT_LABEL, fidelity: "reconstructed" },
    metadata: {
      session_id: first.sessionId,
      cwd: first.cwd,
      ...(first.version ? { harness_version: first.version } : {}),
      ...(skippedSidechain ? { sidechain_entries_skipped: skippedSidechain } : {}),
    },
    spans,
  };
}

export async function importSession(flags) {
  let session = flags.session;
  if (!session) {
    const dir = projectDir(process.cwd());
    session = await latestSession(dir);
    if (!session) {
      console.error(`error: no sessions found in ${dir}`);
      console.error("  pass one explicitly: slink import --session <file.jsonl>");
      process.exit(1);
    }
  }
  let text;
  try {
    text = await readFile(session, "utf8");
  } catch (e) {
    console.error(`error: cannot read ${session}${e?.code ? ` (${e.code})` : ""}`);
    process.exit(1);
  }
  const run = transcriptToRun(
    text.split("\n").filter(Boolean),
    path.basename(session, ".jsonl"),
  );
  if (!run) {
    console.error(`error: no importable messages in ${session}`);
    process.exit(1);
  }
  const file = newCapturePath(new Date(run.created_at));
  await writeRun(file, run);
  const calls = run.spans.filter((s) => s.type === "llm_call").length;
  console.error(
    `imported "${run.name}" — ${calls} llm calls, ${run.spans.length} spans (reconstructed) → ${file}`,
  );
  console.error("  preview: slink open   publish: slink push");
}
