import { spawn } from "node:child_process";
import { chmodSync, closeSync, fchmodSync, mkdirSync, openSync, writeFileSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import type {
  BeforeAgentStartEvent,
  BeforeProviderRequestEvent,
  ExtensionAPI,
  ExtensionCommandContext,
  ExtensionContext,
  SessionShutdownEvent,
  SessionStartEvent,
  TurnEndEvent,
} from "@earendil-works/pi-coding-agent";
import { SessionCapture } from "./capture.mjs";

/**
 * session.link extension for pi — publish the session you're in to a
 * permanent, shareable URL, with exact fidelity.
 *
 * As you work, the extension records each turn from pi's in-process SDK hooks
 * (the assembled system prompt, the verbatim provider request, and the parsed
 * response) into a local session/v0 capture — nothing leaves your machine.
 * `/slink` runs the CLI's publish gate (validate + secret-scan) on that
 * capture and hands you back the URL. When the capture does not hold the
 * whole session (a resumed one had turns before this run), it publishes pi's
 * own transcript through `slink share` instead (reconstructed), so the link
 * always carries the session from its first turn.
 */

// --- CLI bridge: prefer an installed `slink`, else npx. Override with SLINK_BIN.
function invocations(): string[][] {
  const bin = process.env.SLINK_BIN;
  if (bin) return [bin.split(" ")];
  return [["slink"], ["npx", "--yes", "session.link"]];
}

interface RunResult {
  code: number;
  stdout: string;
  stderr: string;
  missing: boolean;
}

function exec(argv: string[]): Promise<RunResult> {
  return new Promise((resolve) => {
    let child;
    try {
      child = spawn(argv[0], argv.slice(1), { stdio: ["ignore", "pipe", "pipe"] });
    } catch {
      resolve({ code: 127, stdout: "", stderr: "", missing: true });
      return;
    }
    let stdout = "";
    let stderr = "";
    child.stdout.on("data", (d) => (stdout += d));
    child.stderr.on("data", (d) => (stderr += d));
    child.on("error", (e: NodeJS.ErrnoException) =>
      resolve({ code: 127, stdout, stderr, missing: e?.code === "ENOENT" }),
    );
    child.on("close", (code) => resolve({ code: code ?? 0, stdout, stderr, missing: false }));
  });
}

async function slink(args: string[]): Promise<RunResult> {
  let last: RunResult = { code: 127, stdout: "", stderr: "", missing: true };
  for (const prefix of invocations()) {
    last = await exec([...prefix, ...args]);
    if (!last.missing) return last;
  }
  return last;
}

const lastLine = (s: string) =>
  s.split("\n").map((l) => l.trim()).filter(Boolean).pop() ?? "";

// The line that says why slink failed. It marks that line with ✗ and follows
// it with details and next steps; when nothing is marked, the last line is it.
const failureLine = (stderr: string) => {
  const marked = stderr.split("\n").map((l) => l.trim()).find((l) => l.startsWith("✗"));
  return (marked ?? lastLine(stderr)).replace(/^✗\s*/, "").replace(/:$/, "");
};

function report(ctx: ExtensionContext, pushed: RunResult): void {
  if (pushed.missing) {
    ctx.ui.notify("session.link: `slink` not found — install it with `npm i -g session.link`", "error");
    return;
  }
  if (pushed.code !== 0) {
    ctx.ui.notify(`session.link: publish failed — ${failureLine(pushed.stderr)}`, "error");
    return;
  }
  const url = lastLine(pushed.stdout);
  ctx.ui.notify(url ? `session.link: published → ${url}` : "session.link: published", "info");
}

// --- capture state (one pi runtime hosts one session at a time)
const nowIso = () => new Date().toISOString();

function newCaptureFile(): string {
  const home = process.env.SLINK_HOME || path.join(os.homedir(), ".slink");
  const dir = path.join(home, "runs");
  mkdirSync(home, { recursive: true, mode: 0o700 });
  chmodSync(home, 0o700);
  mkdirSync(dir, { recursive: true, mode: 0o700 });
  chmodSync(dir, 0o700);
  const d = new Date();
  const p = (n: number) => String(n).padStart(2, "0");
  const stamp = `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}-${p(d.getHours())}${p(d.getMinutes())}${p(d.getSeconds())}-${Math.random().toString(16).slice(2, 8)}`;
  return path.join(dir, `${stamp}.json`);
}

const trunc = (s: string) => (s.length > 80 ? `${s.slice(0, 77)}…` : s);

export default function (pi: ExtensionAPI): void {
  let capture: SessionCapture | null = null;
  let captureFile: string | null = null;
  let startedIso: string | null = null;
  let named = false;
  // Whether the live capture holds the whole session. A session that already
  // had turns when it started here (resumed, forked, reloaded) does not.
  let complete = true;

  // A capture bug must never take down the user's pi session.
  const guard = (fn: () => void) => {
    try {
      fn();
    } catch {
      /* swallow — capture is best-effort */
    }
  };

  const flush = () =>
    guard(() => {
      if (capture && captureFile) {
        const file = openSync(captureFile, "w", 0o600);
        try { fchmodSync(file, 0o600); writeFileSync(file, JSON.stringify(capture.run, null, 2)); }
        finally { closeSync(file); }
      }
    });

  // What to hand slink to publish the whole session, or null when there is
  // nothing yet. The live capture is exact, so it is used whenever it holds
  // every turn. A session that was resumed had turns this run never saw, so
  // that one is imported from the transcript pi keeps (reconstructed).
  const publishArgs = (ctx: ExtensionContext): string[] | null => {
    const sessionFile = ctx.sessionManager.getSessionFile();
    const live = capture && capture.llmCalls > 0 ? captureFile : null;
    if (live && (complete || !sessionFile)) return ["push", "--yes", live];
    return sessionFile ? ["share", "--from", "pi", "--session", sessionFile, "--yes"] : null;
  };

  pi.on("session_start", (_e: SessionStartEvent, ctx: ExtensionContext) =>
    guard(() => {
      capture = new SessionCapture({
        sessionId: ctx.sessionManager.getSessionId(),
        cwd: ctx.cwd,
        name: ctx.sessionManager.getSessionName(),
        startedAtIso: nowIso(),
      });
      captureFile = newCaptureFile();
      named = Boolean(ctx.sessionManager.getSessionName());
      complete = !(ctx.sessionManager.getBranch?.() ?? []).some((entry) => entry.type === "message");
    }),
  );

  pi.on("before_agent_start", (e: BeforeAgentStartEvent) =>
    guard(() => {
      if (!capture) return;
      if (!named && e.prompt) {
        capture.setName(trunc(e.prompt));
        named = true;
      }
      capture.beforeAgent({ systemPrompt: e.systemPrompt, prompt: e.prompt, images: e.images });
      startedIso = nowIso();
    }),
  );

  pi.on("before_provider_request", (e: BeforeProviderRequestEvent) =>
    guard(() => {
      capture?.providerRequest(e.payload);
      startedIso = nowIso();
    }),
  );

  pi.on("turn_end", (e: TurnEndEvent) =>
    guard(() => {
      if (!capture) return;
      capture.turnEnd({
        message: e.message,
        toolResults: e.toolResults,
        startedIso: startedIso ?? nowIso(),
        endedIso: nowIso(),
      });
      startedIso = null;
      flush();
    }),
  );

  pi.on("session_shutdown", async (e: SessionShutdownEvent, ctx: ExtensionContext): Promise<void> => {
    try {
      capture?.finalize(nowIso());
      flush();
      // Opt-in (SLINK_AUTOPUBLISH): when the session actually ends, hand back
      // a link — the semi-auto "the agent published its own trace" flow. Only
      // on a real quit, and only if something was captured.
      if (process.env.SLINK_AUTOPUBLISH && e.reason === "quit" && capture && capture.llmCalls > 0 && captureFile) {
        report(ctx, await slink(publishArgs(ctx)!));
      }
    } catch {
      /* capture + auto-publish are both best-effort */
    }
  });

  pi.registerCommand("slink", {
    description: "Publish this session, or use /slink view for a local web preview",
    handler: async (args: string, ctx: ExtensionCommandContext): Promise<void> => {
      if (args.trim() === "view") {
        // Prefer the persisted transcript so a resumed session includes its
        // earlier turns. Pin the path: never fall back to another recent session.
        const sessionFile = ctx.sessionManager.getSessionFile();
        flush();
        const source = sessionFile || (capture && capture.llmCalls > 0 ? captureFile : null);
        if (!source) {
          ctx.ui.notify("session.link: no session to preview yet", "error");
          return;
        }
        ctx.ui.notify("session.link: opening a local preview…", "info");
        const result = await slink(["view", "--from", "pi", "--session", source, "--background"]);
        if (result.missing) {
          ctx.ui.notify("session.link: `slink` not found — install it with `npm i -g session.link`", "error");
        } else if (result.code !== 0) {
          ctx.ui.notify(`session.link: preview failed — ${result.stderr.trim()}`, "error");
        } else {
          ctx.ui.notify(`session.link: local preview → ${lastLine(result.stdout)} (nothing uploaded)`, "info");
        }
        return;
      }
      if (args.trim()) {
        ctx.ui.notify("session.link: use /slink to publish, or /slink view to preview locally", "error");
        return;
      }
      const publish = publishArgs(ctx);
      if (!publish) {
        ctx.ui.notify("session.link: no session to publish yet", "error");
        return;
      }
      if (publish[0] === "push") {
        flush(); // snapshot; the run keeps accumulating after this
        ctx.ui.notify("session.link: publishing this session…", "info");
      } else {
        ctx.ui.notify("session.link: capturing this session…", "info");
      }
      report(ctx, await slink(publish));
    },
  });
}
