/**
 * Live pi events → a session/v0 run, built as the agent runs.
 *
 * This is the exact-capture core, kept pure (no pi imports, no I/O) so it can
 * be unit-tested with plain Node: index.ts feeds it typed pi events and
 * timestamps, it returns spans. Fidelity is "exact" — the run is assembled
 * from pi's in-process SDK hooks at call time (source.kind "sdk"), and the
 * verbatim provider request payload rides along in span.raw.request, so the
 * exact bytes are recoverable even though the normalized view is derived from
 * pi's already-parsed messages.
 *
 * pi content parts (from @earendil-works/pi-ai): text {text}, thinking
 * {thinking}, toolCall {id,name,arguments}; tool results are their own
 * message with {toolCallId, content}. Usage is {input, output, cacheRead,…}.
 */

const dataPart = (data) => ({ type: "data", data });

function mapPart(block) {
  if (!block || typeof block.type !== "string") return dataPart(block);
  switch (block.type) {
    case "text":
      return { type: "text", text: block.text ?? "" };
    case "thinking":
      // Redacted thinking has no plaintext; keep a marker, drop the opaque blob.
      return { type: "thinking", text: block.redacted ? "[redacted]" : block.thinking ?? "" };
    case "toolCall":
      if (typeof block.id !== "string" || typeof block.name !== "string") return dataPart(block);
      return { type: "tool_call", id: block.id, name: block.name, arguments: block.arguments };
    case "image":
      // Inline bytes would balloon the normalized view; note it, bytes stay in raw.
      return dataPart({ kind: "image", mime: block.mimeType, note: "inline bytes omitted; see raw" });
    default:
      return block; // open-world pass-through
  }
}

function mapContent(content) {
  if (content == null) return [];
  if (typeof content === "string") return [{ type: "text", text: content }];
  if (!Array.isArray(content)) return [dataPart(content)];
  return content.map(mapPart);
}

function mapUsage(u) {
  if (!u) return undefined;
  const out = {};
  if (u.input != null) out.input_tokens = u.input;
  if (u.output != null) out.output_tokens = u.output;
  if (u.cacheRead != null) out.cache_read_tokens = u.cacheRead;
  if (u.reasoning != null) out.reasoning_tokens = u.reasoning;
  return Object.keys(out).length ? out : undefined;
}

const toolResultValue = (parts) =>
  parts.length === 1 && parts[0]?.type === "text" ? parts[0].text : parts;

export class SessionCapture {
  /** meta: { name?, sessionId?, cwd?, startedAtIso } */
  constructor(meta) {
    const name = meta.name || meta.sessionId || "pi session";
    this.seq = 0;
    this.pending = []; // input delta (user + tool messages) since the last call
    this.system = undefined; // latest assembled system prompt
    this.rawRequest = null; // verbatim payload from before_provider_request
    this.openTools = new Map(); // toolCallId -> tool_call span
    this.run = {
      schema: "session/v0",
      name,
      created_at: meta.startedAtIso,
      source: { kind: "sdk", harness: "pi", label: "pi-extension@0.1.0", fidelity: "exact" },
      metadata: {
        ...(meta.sessionId ? { session_id: meta.sessionId } : {}),
        ...(meta.cwd ? { cwd: meta.cwd } : {}),
        in_progress: true,
      },
      spans: [
        { id: "root", parent_id: null, type: "agent", name, started_at: meta.startedAtIso },
      ],
    };
  }

  setName(name) {
    if (name) {
      this.run.name = name;
      this.run.spans[0].name = name;
    }
  }

  /** before_agent_start: assembled system prompt + the user's prompt for this loop. */
  beforeAgent({ systemPrompt, prompt, images }) {
    if (systemPrompt) this.system = systemPrompt;
    const content = [];
    if (typeof prompt === "string" && prompt) content.push({ type: "text", text: prompt });
    for (const im of images ?? []) content.push(mapPart(im));
    if (content.length) this.pending.push({ role: "user", content });
  }

  /** before_provider_request: stash the exact request body for the next span's raw. */
  providerRequest(payload) {
    this.rawRequest = payload ?? null;
  }

  /** turn_end: one assistant message + its tool results become an llm_call (+ tool spans). */
  turnEnd({ message, toolResults, startedIso, endedIso }) {
    const span = {
      id: `s${++this.seq}`,
      parent_id: "root",
      type: "llm_call",
      name: "turn",
      started_at: startedIso,
      ended_at: endedIso,
      status: message?.stopReason === "error" ? "error" : "ok",
      model: {
        id: String(message?.model ?? "unknown"),
        provider: String(message?.provider ?? "unknown"),
      },
      input: { ...(this.system ? { system: this.system } : {}), messages: this.pending },
      output: { messages: [{ role: "assistant", content: mapContent(message?.content) }] },
      raw: { request: this.rawRequest ?? null },
    };
    if (message?.errorMessage) span.error = { message: message.errorMessage };
    const usage = mapUsage(message?.usage);
    if (usage) span.usage = usage;
    this.run.spans.push(span);
    this.pending = [];
    this.rawRequest = null;

    for (const block of Array.isArray(message?.content) ? message.content : []) {
      if (block?.type === "toolCall" && typeof block.id === "string") {
        const toolSpan = {
          id: `s${++this.seq}`,
          parent_id: span.id,
          type: "tool_call",
          name: block.name,
          started_at: endedIso,
          status: "ok",
          input: { name: block.name, arguments: block.arguments, tool_call_id: block.id },
        };
        this.run.spans.push(toolSpan);
        this.openTools.set(block.id, toolSpan);
      }
    }

    for (const tr of toolResults ?? []) {
      const parts = mapContent(tr.content);
      const toolSpan = this.openTools.get(tr.toolCallId);
      if (toolSpan) {
        toolSpan.ended_at = endedIso;
        toolSpan.output = {
          result: toolResultValue(parts),
          ...(tr.isError ? { is_error: true } : {}),
        };
        if (tr.isError) toolSpan.status = "error";
        this.openTools.delete(tr.toolCallId);
      }
      // The model sees the result on its next call — keep it in the delta.
      this.pending.push({
        role: "tool",
        content: [
          {
            type: "tool_result",
            tool_call_id: tr.toolCallId,
            ...(tr.isError ? { is_error: true } : {}),
            content: parts,
          },
        ],
      });
    }
  }

  /** Close the run. Any un-answered trailing messages survive as a custom span. */
  finalize(endedIso) {
    const root = this.run.spans[0];
    root.ended_at = endedIso;
    root.status = "ok";
    delete this.run.metadata.in_progress;
    if (this.pending.length) {
      this.run.spans.push({
        id: `s${++this.seq}`,
        parent_id: "root",
        type: "custom",
        name: "trailing message(s) — no reply captured",
        started_at: endedIso,
        ended_at: endedIso,
        input: { messages: this.pending },
      });
      this.pending = [];
    }
    return this.run;
  }

  get llmCalls() {
    return this.run.spans.filter((s) => s.type === "llm_call").length;
  }
}
