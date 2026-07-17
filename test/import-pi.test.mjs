import assert from "node:assert/strict";
import { test } from "node:test";
import { piSessionToRun, looksLikePi, piProjectDir } from "../cli/import-pi.mjs";
import { validateRun } from "@session-link/format/validate";

const line = (o) => JSON.stringify(o);
const ts = (n) => `2026-07-10T10:00:${String(n).padStart(2, "0")}.000Z`;
const SID = "019da2d0-d12c-7237-95b4-4ceca6f6b894";

const session = () => line({ type: "session", version: 3, id: SID, timestamp: ts(0), cwd: "/tmp/p" });
const message = (n, msg) => line({ type: "message", id: `m${n}`, parentId: `m${n - 1}`, timestamp: ts(n), message: msg });

test("piSessionToRun: names from the opener, carries pi provenance + per-message model/usage", () => {
  const run = piSessionToRun([
    session(),
    message(1, { role: "user", content: [{ type: "text", text: "add a rate limiter" }] }),
    message(2, {
      role: "assistant",
      provider: "openai-codex",
      model: "gpt-5.4",
      content: [{ type: "text", text: "on it" }],
      usage: { input: 812, output: 190, cacheRead: 100, totalTokens: 1002 },
    }),
  ]);
  assert.equal(run.name, "add a rate limiter");
  assert.deepEqual(run.source, {
    kind: "import",
    harness: "pi",
    label: "pi-import@0.1.0",
    fidelity: "reconstructed",
  });
  assert.equal(run.metadata.session_id, SID);
  assert.equal(run.metadata.cwd, "/tmp/p");
  const llm = run.spans.find((s) => s.type === "llm_call");
  assert.deepEqual(llm.model, { id: "gpt-5.4", provider: "openai-codex" });
  assert.equal(llm.input.messages[0].content[0].text, "add a rate limiter");
  assert.deepEqual(llm.usage, { input_tokens: 812, output_tokens: 190, cache_read_tokens: 100 });
});

test("piSessionToRun: toolCall pairs with toolResult and feeds the next turn's input", () => {
  const run = piSessionToRun([
    session(),
    message(1, { role: "user", content: [{ type: "text", text: "read the file" }] }),
    message(2, {
      role: "assistant",
      provider: "anthropic",
      model: "claude-x",
      content: [{ type: "toolCall", id: "call_1|fc_9", name: "read", arguments: { path: "SKILL.md" } }],
    }),
    message(3, {
      role: "toolResult",
      toolCallId: "call_1|fc_9",
      toolName: "read",
      content: [{ type: "text", text: "file contents here" }],
    }),
    message(4, {
      role: "assistant",
      provider: "anthropic",
      model: "claude-x",
      content: [{ type: "text", text: "done" }],
    }),
  ]);
  const tool = run.spans.find((s) => s.type === "tool_call");
  assert.equal(tool.name, "read");
  assert.equal(tool.parent_id, run.spans.find((s) => s.type === "llm_call").id);
  assert.equal(tool.ended_at, ts(3));
  assert.deepEqual(tool.input.arguments, { path: "SKILL.md" });
  assert.equal(tool.output.result, "file contents here");
  assert.equal(tool.status, "ok");
  // the tool result becomes a tool-role message in the *next* llm_call's input
  const secondCall = run.spans.filter((s) => s.type === "llm_call")[1];
  const toolMsg = secondCall.input.messages.find((m) => m.role === "tool");
  assert.ok(toolMsg, "tool result must be replayed into the next call's input");
  assert.equal(toolMsg.content[0].type, "tool_result");
  assert.equal(toolMsg.content[0].tool_call_id, "call_1|fc_9");
});

test("piSessionToRun: error tool result marks the tool span", () => {
  const run = piSessionToRun([
    session(),
    message(1, { role: "user", content: [{ type: "text", text: "go" }] }),
    message(2, {
      role: "assistant",
      provider: "anthropic",
      model: "claude-x",
      content: [{ type: "toolCall", id: "t1", name: "bash", arguments: {} }],
    }),
    message(3, { role: "toolResult", toolCallId: "t1", toolName: "bash", isError: true, content: [{ type: "text", text: "boom" }] }),
    message(4, { role: "assistant", provider: "anthropic", model: "claude-x", content: [{ type: "text", text: "hm" }] }),
  ]);
  assert.equal(run.spans.find((s) => s.type === "tool_call").status, "error");
});

test("piSessionToRun: thinking parts normalize to thinking text", () => {
  const run = piSessionToRun([
    session(),
    message(1, { role: "user", content: [{ type: "text", text: "hi" }] }),
    message(2, {
      role: "assistant",
      provider: "openai-codex",
      model: "gpt-5.4",
      content: [
        { type: "thinking", thinking: "let me consider", thinkingSignature: "{opaque}" },
        { type: "text", text: "hello" },
      ],
    }),
  ]);
  const out = run.spans.find((s) => s.type === "llm_call").output.messages[0].content;
  assert.deepEqual(out[0], { type: "thinking", text: "let me consider" });
  assert.deepEqual(out[1], { type: "text", text: "hello" });
});

test("piSessionToRun: trailing user turn survives as a custom span", () => {
  const run = piSessionToRun([
    session(),
    message(1, { role: "user", content: [{ type: "text", text: "q" }] }),
    message(2, { role: "assistant", provider: "anthropic", model: "claude-x", content: [{ type: "text", text: "a" }] }),
    message(3, { role: "user", content: [{ type: "text", text: "one more thing" }] }),
  ]);
  const tail = run.spans.find((s) => s.type === "custom");
  assert.ok(tail, "trailing user message must not be dropped");
  assert.equal(tail.input.messages[0].content[0].text, "one more thing");
});

test("piSessionToRun: empty / no messages returns null", () => {
  assert.equal(piSessionToRun([session()]), null);
  assert.equal(piSessionToRun(["not json"]), null);
});

test("imported pi runs validate against session/v0", () => {
  const run = piSessionToRun([
    session(),
    message(1, { role: "user", content: [{ type: "text", text: "read it" }] }),
    message(2, {
      role: "assistant",
      provider: "openai-codex",
      model: "gpt-5.4",
      content: [
        { type: "thinking", thinking: "hmm" },
        { type: "toolCall", id: "t1", name: "read", arguments: { path: "x" } },
      ],
      usage: { input: 10, output: 4 },
    }),
    message(3, { role: "toolResult", toolCallId: "t1", toolName: "read", content: [{ type: "text", text: "ok" }] }),
    message(4, { role: "user", content: [{ type: "text", text: "trailing" }] }),
  ]);
  assert.deepEqual(validateRun(run), []);
});

test("looksLikePi distinguishes pi from Claude Code entries", () => {
  assert.equal(looksLikePi([{ type: "session" }, { type: "message" }]), true);
  assert.equal(looksLikePi([{ type: "user" }, { type: "assistant" }]), false);
  assert.equal(looksLikePi([{ type: "summary" }]), false);
});

test("piProjectDir encodes cwd the way pi does", () => {
  assert.ok(piProjectDir("/Users/e/code/fastrand").endsWith("--Users-e-code-fastrand--"));
  assert.ok(piProjectDir("/Users/e/code/radicle.dev").endsWith("--Users-e-code-radicle.dev--"));
});
