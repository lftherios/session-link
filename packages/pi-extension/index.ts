import { spawn } from "node:child_process";
import { mkdirSync, writeFileSync } from "node:fs";
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
 * capture and hands you back the URL. If there's no live capture yet (e.g. a
 * resumed session), it falls back to `slink import` (reconstructed) so the
 * command always works.
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

function report(ctx: ExtensionContext, pushed: RunResult): void {
  if (pushed.missing) {
    ctx.ui.notify("session.link: `slink` not found — install it with `npm i -g session.link`", "error");
    return;
  }
  if (pushed.code !== 0) {
    ctx.ui.notify(`session.link: publish failed — ${lastLine(pushed.stderr)}`, "error");
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
  mkdirSync(dir, { recursive: true });
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
      if (capture && captureFile) writeFileSync(captureFile, JSON.stringify(capture.run, null, 2));
    });

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

  pi.on("session_shutdown", (_e: SessionShutdownEvent) =>
    guard(() => {
      capture?.finalize(nowIso());
      flush();
    }),
  );

  pi.registerCommand("slink", {
    description: "Publish the current pi session to a session.link URL",
    handler: async (_args: string, ctx: ExtensionCommandContext): Promise<void> => {
      // Preferred path: publish the live, exact capture we've been building.
      if (capture && capture.llmCalls > 0 && captureFile) {
        flush(); // snapshot; the run keeps accumulating after this
        ctx.ui.notify("session.link: publishing this session…", "info");
        report(ctx, await slink(["push", "--yes", captureFile]));
        return;
      }

      // Fallback: no live turns yet (e.g. resumed session) — import from disk.
      const sessionFile = ctx.sessionManager.getSessionFile();
      if (!sessionFile) {
        ctx.ui.notify("session.link: no session to publish yet", "error");
        return;
      }
      ctx.ui.notify("session.link: capturing this session…", "info");
      const imported = await slink(["import", "--from", "pi", "--session", sessionFile]);
      if (imported.missing) {
        ctx.ui.notify("session.link: `slink` not found — install it with `npm i -g session.link`", "error");
        return;
      }
      if (imported.code !== 0) {
        ctx.ui.notify(`session.link: capture failed — ${lastLine(imported.stderr)}`, "error");
        return;
      }
      const capturePath = lastLine(imported.stdout);
      if (!capturePath) {
        ctx.ui.notify("session.link: capture produced no file", "error");
        return;
      }
      ctx.ui.notify("session.link: publishing…", "info");
      report(ctx, await slink(["push", "--yes", capturePath]));
    },
  });
}
