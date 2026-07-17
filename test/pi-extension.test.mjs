import assert from "node:assert/strict";
import { test } from "node:test";
import { SessionCapture } from "../packages/pi-extension/capture.mjs";
import { validateRun } from "@session-link/format/validate";

const iso = (n) => `2026-07-16T10:00:${String(n).padStart(2, "0")}.000Z`;

// pi-ai-shaped message builders
const assistant = (content, extra = {}) => ({
  role: "assistant",
  content,
  provider: "openai-codex",
  model: "gpt-5.4",
  usage: { input: 812, output: 190, cacheRead: 0, cacheWrite: 0, totalTokens: 1002, reasoning: 40 },
  stopReason: "toolUse",
  timestamp: 1,
  ...extra,
});
const toolResult = (toolCallId, text, extra = {}) => ({
  role: "toolResult",
  toolCallId,
  toolName: "read",
  content: [{ type: "text", text }],
  isError: false,
  timestamp: 2,
  ...extra,
});

function fresh() {
  return new SessionCapture({ sessionId: "sess-1", cwd: "/tmp/p", startedAtIso: iso(0) });
}

test("SessionCapture: exact fidelity, sdk kind, system prompt + verbatim raw request", () => {
  const cap = fresh();
  cap.beforeAgent({ systemPrompt: "you are helpful", prompt: "read the file" });
  const rawReq = { model: "gpt-5.4", messages: [{ role: "user", content: "read the file" }], temperature: 0.2 };
  cap.providerRequest(rawReq);
  cap.turnEnd({
    message: assistant([
      { type: "thinking", thinking: "let me look" },
      { type: "toolCall", id: "c1", name: "read", arguments: { path: "x" } },
    ]),
    toolResults: [],
    startedIso: iso(1),
    endedIso: iso(2),
  });

  assert.deepEqual(cap.run.source, {
    kind: "sdk",
    harness: "pi",
    label: "pi-extension@0.1.0",
    fidelity: "exact",
  });
  const llm = cap.run.spans.find((s) => s.type === "llm_call");
  assert.equal(llm.input.system, "you are helpful");
  assert.deepEqual(llm.raw.request, rawReq, "exact request payload rides verbatim in raw");
  assert.deepEqual(llm.model, { id: "gpt-5.4", provider: "openai-codex" });
  assert.deepEqual(llm.usage, { input_tokens: 812, output_tokens: 190, cache_read_tokens: 0, reasoning_tokens: 40 });
  assert.equal(llm.input.messages[0].content[0].text, "read the file");
  assert.deepEqual(llm.output.messages[0].content[0], { type: "thinking", text: "let me look" });
});

test("SessionCapture: tool call pairs with result and feeds the next turn's input", () => {
  const cap = fresh();
  cap.beforeAgent({ systemPrompt: "sp", prompt: "go" });
  cap.providerRequest({ model: "gpt-5.4" });
  cap.turnEnd({
    message: assistant([{ type: "toolCall", id: "c1", name: "read", arguments: { path: "x" } }]),
    toolResults: [toolResult("c1", "file contents")],
    startedIso: iso(1),
    endedIso: iso(2),
  });
  cap.providerRequest({ model: "gpt-5.4" });
  cap.turnEnd({
    message: assistant([{ type: "text", text: "done" }], { stopReason: "stop" }),
    toolResults: [],
    startedIso: iso(3),
    endedIso: iso(4),
  });

  const tool = cap.run.spans.find((s) => s.type === "tool_call");
  assert.equal(tool.name, "read");
  assert.equal(tool.parent_id, cap.run.spans.find((s) => s.type === "llm_call").id);
  assert.equal(tool.ended_at, iso(2));
  assert.equal(tool.output.result, "file contents");
  const secondCall = cap.run.spans.filter((s) => s.type === "llm_call")[1];
  const toolMsg = secondCall.input.messages.find((m) => m.role === "tool");
  assert.ok(toolMsg, "tool result replayed into the next call's input");
  assert.equal(toolMsg.content[0].tool_call_id, "c1");
});

test("SessionCapture: error stop reason marks the span; error tool result marks its span", () => {
  const cap = fresh();
  cap.beforeAgent({ systemPrompt: "sp", prompt: "go" });
  cap.providerRequest({});
  cap.turnEnd({
    message: assistant([{ type: "toolCall", id: "c1", name: "bash", arguments: {} }], {
      stopReason: "error",
      errorMessage: "boom",
    }),
    toolResults: [toolResult("c1", "nope", { isError: true })],
    startedIso: iso(1),
    endedIso: iso(2),
  });
  const llm = cap.run.spans.find((s) => s.type === "llm_call");
  assert.equal(llm.status, "error");
  assert.deepEqual(llm.error, { message: "boom" });
  assert.equal(cap.run.spans.find((s) => s.type === "tool_call").status, "error");
});

test("SessionCapture: redacted thinking is masked, not leaked", () => {
  const cap = fresh();
  cap.beforeAgent({ systemPrompt: "sp", prompt: "hi" });
  cap.providerRequest({});
  cap.turnEnd({
    message: assistant([{ type: "thinking", thinking: "secret", redacted: true, thinkingSignature: "opaque" }]),
    toolResults: [],
    startedIso: iso(1),
    endedIso: iso(2),
  });
  const out = cap.run.spans.find((s) => s.type === "llm_call").output.messages[0].content[0];
  assert.deepEqual(out, { type: "thinking", text: "[redacted]" });
});

test("SessionCapture: name derives from... setName; finalize clears in_progress", () => {
  const cap = fresh();
  cap.setName("add a rate limiter");
  cap.beforeAgent({ systemPrompt: "sp", prompt: "add a rate limiter" });
  cap.providerRequest({});
  cap.turnEnd({ message: assistant([{ type: "text", text: "ok" }], { stopReason: "stop" }), toolResults: [], startedIso: iso(1), endedIso: iso(2) });
  assert.equal(cap.run.metadata.in_progress, true);
  const run = cap.finalize(iso(3));
  assert.equal(run.name, "add a rate limiter");
  assert.equal(run.metadata.in_progress, undefined);
  assert.equal(run.spans[0].ended_at, iso(3));
});

test("SessionCapture: a trailing user prompt with no reply survives as a custom span", () => {
  const cap = fresh();
  cap.beforeAgent({ systemPrompt: "sp", prompt: "one more thing" });
  const run = cap.finalize(iso(1));
  const tail = run.spans.find((s) => s.type === "custom");
  assert.ok(tail, "trailing user message must not be dropped");
  assert.equal(tail.input.messages[0].content[0].text, "one more thing");
});

test("captured runs validate against session/v0", () => {
  const cap = fresh();
  cap.beforeAgent({ systemPrompt: "sp", prompt: "read it" });
  cap.providerRequest({ model: "gpt-5.4", messages: [] });
  cap.turnEnd({
    message: assistant([
      { type: "thinking", thinking: "hmm" },
      { type: "toolCall", id: "c1", name: "read", arguments: { path: "x" } },
    ]),
    toolResults: [toolResult("c1", "ok")],
    startedIso: iso(1),
    endedIso: iso(2),
  });
  cap.providerRequest({ model: "gpt-5.4" });
  cap.turnEnd({ message: assistant([{ type: "text", text: "done" }], { stopReason: "stop" }), toolResults: [], startedIso: iso(3), endedIso: iso(4) });
  const run = cap.finalize(iso(5));
  assert.deepEqual(validateRun(run), []);
});
