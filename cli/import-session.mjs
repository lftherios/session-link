import { readdir, readFile, stat } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { newCapturePath, writeRun } from "./store.mjs";
import { anthropicContent, anthropicUsage } from "./normalize.mjs";
import { piProjectDir, piSessionToRun, looksLikePi } from "./import-pi.mjs";
import { latestOpencode, loadOpencode } from "./import-opencode.mjs";
import { latestHermes, loadHermes } from "./import-hermes.mjs";
import { codexRolloutToRun, looksLikeCodex, latestCodex } from "./import-codex.mjs";

/**
 * Retroactive importers: turn a coding-agent session transcript into a
 * session/v0 capture — works with zero prior setup, for the session you
 * already had. One adapter per harness (Claude Code, pi, …); `slink import`
 * auto-detects which store to read, or `--from <harness>` forces one.
 *
 * Reconstructed, not wire capture: a transcript has messages, tool
 * calls/results, models, and usage, but not the assembled system prompt or
 * per-call request bodies — so runs are marked fidelity: "reconstructed".
 */

const IMPORT_LABEL = "session-import@0.1.0";

/* ------------------------------------------------------ Claude Code adapter */

function claudeProjectDir(cwd) {
  return path.join(
    os.homedir(),
    ".claude",
    "projects",
    cwd.replaceAll("/", "-").replaceAll(".", "-"),
  );
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
    source: { kind: "import", harness: "claude-code", label: IMPORT_LABEL, fidelity: "reconstructed" },
    metadata: {
      session_id: first.sessionId,
      cwd: first.cwd,
      ...(first.version ? { harness_version: first.version } : {}),
      ...(skippedSidechain ? { sidechain_entries_skipped: skippedSidechain } : {}),
    },
    spans,
  };
}

/* ----------------------------------------------------------- the registry */

/** Newest *.jsonl in dir, by mtime — or null if the dir is missing/empty. */
async function newestSession(dir) {
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
    if (!best || s.mtimeMs > best.mtimeMs) best = { file: full, mtimeMs: s.mtimeMs };
  }
  return best;
}

async function parseFile(file, parse) {
  const text = await readFile(file, "utf8");
  return parse(text.split("\n").filter(Boolean), path.basename(file, ".jsonl"));
}

/**
 * A harness that stores sessions as per-cwd JSONL files. Exposes a uniform
 * interface (latest/fromFile) alongside the DB-backed importers below.
 */
function fileImporter({ name, projectDir, parse, detect }) {
  return {
    name,
    detect,
    parse,
    latest: async (cwd) => {
      const found = await newestSession(projectDir(cwd));
      return found ? { recencyMs: found.mtimeMs, load: () => parseFile(found.file, parse) } : null;
    },
    fromFile: (file) => parseFile(file, parse),
  };
}

const IMPORTERS = [
  fileImporter({
    name: "claude-code",
    projectDir: claudeProjectDir,
    parse: transcriptToRun,
    // CC entries are top-level type user/assistant/summary; pi never is.
    detect: (entries) => entries.some((e) => e?.type === "user" || e?.type === "assistant"),
  }),
  fileImporter({
    name: "pi",
    projectDir: piProjectDir,
    parse: piSessionToRun,
    detect: looksLikePi,
  }),
  {
    // opencode stores sessions in one SQLite DB, not per-cwd files; --session
    // takes a session id, and latest/fromExplicit read the DB directly.
    name: "opencode",
    db: true,
    latest: latestOpencode,
    fromExplicit: loadOpencode,
  },
  {
    // hermes: same shape as opencode — one SQLite state.db, id-addressed.
    name: "hermes",
    db: true,
    latest: latestHermes,
    fromExplicit: loadHermes,
  },
  {
    // codex writes JSONL rollouts in a date tree; parse handles a rollout
    // file, latest scans the tree matching session_meta.cwd.
    name: "codex",
    parse: codexRolloutToRun,
    detect: looksLikeCodex,
    latest: latestCodex,
  },
];

/** Parse the first few non-empty lines, for format sniffing. */
function peek(text, n = 20) {
  const out = [];
  for (const line of text.split("\n")) {
    if (!line.trim()) continue;
    try {
      out.push(JSON.parse(line));
    } catch {
      /* skip */
    }
    if (out.length >= n) break;
  }
  return out;
}

function die(msg, ...more) {
  console.error(`error: ${msg}`);
  for (const m of more) console.error(m);
  process.exit(1);
}

export async function importSession(flags) {
  let importer = null;
  if (flags.from) {
    importer = IMPORTERS.find((i) => i.name === flags.from);
    if (!importer)
      die(
        `unknown --from "${flags.from}"`,
        `  known harnesses: ${IMPORTERS.map((i) => i.name).join(", ")}`,
      );
  }

  let run;

  if (flags.session) {
    if (importer?.db) {
      // opencode: --session is a session id, resolved against the DB.
      try {
        run = await importer.fromExplicit(flags.session);
      } catch (e) {
        die(e?.message ?? String(e));
      }
    } else {
      // A transcript file: read it, then use the chosen or sniffed importer.
      let text;
      try {
        text = await readFile(flags.session, "utf8");
      } catch (e) {
        die(`cannot read ${flags.session}${e?.code ? ` (${e.code})` : ""}`);
      }
      let fileImp = importer;
      if (!fileImp) {
        fileImp = IMPORTERS.find((i) => i.detect && i.detect(peek(text)));
        if (!fileImp)
          die(
            `could not recognize the transcript format of ${flags.session}`,
            `  force it: slink import --from ${IMPORTERS.map((i) => i.name).join("|")} <file>`,
          );
      }
      if (!fileImp.parse)
        die(`--from ${fileImp.name} reads no file — drop --session and it auto-locates the latest ${fileImp.name} session`);
      run = fileImp.parse(text.split("\n").filter(Boolean), path.basename(flags.session, ".jsonl"));
    }
  } else {
    // Auto-locate the most recent session for this cwd, across the chosen
    // harness or all of them. Each importer's latest() is best-effort — a
    // missing store (or no node:sqlite) just means that harness has nothing.
    const cwd = process.cwd();
    const candidates = importer ? [importer] : IMPORTERS;
    let best = null;
    for (const imp of candidates) {
      let found = null;
      try {
        found = await imp.latest(cwd);
      } catch {
        found = null;
      }
      if (found && (!best || found.recencyMs > best.recencyMs)) best = found;
    }
    if (!best)
      die(
        `no ${importer ? `${importer.name} ` : ""}sessions found for ${cwd}`,
        "  pass one explicitly: slink import --session <file.jsonl|id>, or --from <harness>",
      );
    run = await best.load();
  }

  if (!run) die("no importable messages");
  const file = newCapturePath(new Date(run.created_at));
  await writeRun(file, run);
  const calls = run.spans.filter((s) => s.type === "llm_call").length;
  console.error(
    `imported "${run.name}" from ${run.source.harness} — ${calls} llm calls, ${run.spans.length} spans (reconstructed) → ${file}`,
  );
  console.error("  preview: slink open   publish: slink push");
  console.log(file); // stdout: the one pipeable line — the capture path
}
