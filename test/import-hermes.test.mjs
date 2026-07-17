import assert from "node:assert/strict";
import { test } from "node:test";
import { buildHermesRun } from "../cli/import-hermes.mjs";
import { validateRun } from "@session-link/format/validate";

const SESSION = {
  id: "sess-1",
  title: "Fix the bug",
  model: "gpt-5.6",
  billing_provider: "openai",
  system_prompt: "You are Hermes.",
  cwd: "/tmp/p",
  source: "cli",
  started_at: 1000,
  ended_at: 1010,
};
const msg = (role, extra) => ({ role, content: null, tool_call_id: null, tool_calls: null, tool_name: null, timestamp: 1001, token_count: null, finish_reason: null, reasoning: null, reasoning_content: null, ...extra });

test("buildHermesRun: basic turn — provenance, model, system prompt, usage, name", () => {
  const run = buildHermesRun(SESSION, [
    msg("user", { content: "fix it", timestamp: 1001 }),
    msg("assistant", { content: "done", timestamp: 1002, token_count: 20, finish_reason: "stop" }),
  ]);
  assert.equal(run.name, "Fix the bug");
  assert.deepEqual(run.source, { kind: "import", harness: "hermes", label: "session-import@0.1.0", fidelity: "reconstructed" });
  assert.equal(run.metadata.session_id, "sess-1");
  assert.equal(run.metadata.cwd, "/tmp/p");
  assert.equal(run.metadata.origin, "cli");
  const llm = run.spans.find((s) => s.type === "llm_call");
  assert.deepEqual(llm.model, { id: "gpt-5.6", provider: "openai" });
  assert.equal(llm.input.system, "You are Hermes.");
  assert.equal(llm.input.messages[0].content[0].text, "fix it");
  assert.equal(llm.output.messages[0].content[0].text, "done");
  assert.deepEqual(llm.usage, { output_tokens: 20 });
});

test("buildHermesRun: OpenAI-shaped tool_calls become a tool_call span + result feeds next input", () => {
  const run = buildHermesRun(SESSION, [
    msg("user", { content: "list files", timestamp: 1001 }),
    msg("assistant", {
      content: null,
      timestamp: 1002,
      tool_calls: JSON.stringify([{ id: "call_1", type: "function", function: { name: "bash", arguments: '{"command":"ls"}' } }]),
    }),
    msg("tool", { content: "a\nb", tool_call_id: "call_1", tool_name: "bash", timestamp: 1003 }),
    msg("assistant", { content: "there they are", timestamp: 1004, finish_reason: "stop" }),
  ]);
  const tool = run.spans.find((s) => s.type === "tool_call");
  assert.equal(tool.name, "bash");
  assert.equal(tool.parent_id, run.spans.find((s) => s.type === "llm_call").id);
  assert.deepEqual(tool.input.arguments, { command: "ls" });
  assert.equal(tool.input.tool_call_id, "call_1");
  assert.equal(tool.output.result, "a\nb");
  assert.equal(tool.ended_at, new Date(1003 * 1000).toISOString());
  // tool call also surfaces as a content part in the assistant output
  const part = run.spans.find((s) => s.type === "llm_call").output.messages[0].content.find((c) => c.type === "tool_call");
  assert.equal(part.id, "call_1");
  // and the tool result is replayed into the next call's input
  const secondCall = run.spans.filter((s) => s.type === "llm_call")[1];
  assert.ok(secondCall.input.messages.some((m) => m.role === "tool" && m.content[0].tool_call_id === "call_1"));
});

test("buildHermesRun: reasoning maps to thinking; finish_reason error marks the span", () => {
  const run = buildHermesRun(SESSION, [
    msg("user", { content: "think", timestamp: 1001 }),
    msg("assistant", { content: "answer", reasoning: "let me reason", finish_reason: "error", timestamp: 1002 }),
  ]);
  const llm = run.spans.find((s) => s.type === "llm_call");
  assert.equal(llm.status, "error");
  assert.deepEqual(llm.output.messages[0].content[0], { type: "thinking", text: "let me reason" });
});

test("buildHermesRun: JSON-list content is parsed into parts; system messages are skipped", () => {
  const run = buildHermesRun(SESSION, [
    msg("system", { content: "ignored inline system", timestamp: 1001 }),
    msg("user", { content: JSON.stringify([{ type: "text", text: "hello" }]), timestamp: 1002 }),
    msg("assistant", { content: "hi", timestamp: 1003, finish_reason: "stop" }),
  ]);
  const llm = run.spans.find((s) => s.type === "llm_call");
  assert.equal(llm.input.messages[0].content[0].text, "hello");
  // exactly one user message (system skipped)
  assert.equal(llm.input.messages.length, 1);
});

test("buildHermesRun: a trailing user message survives as a custom span", () => {
  const run = buildHermesRun(SESSION, [msg("user", { content: "one more thing", timestamp: 1001 })]);
  const tail = run.spans.find((s) => s.type === "custom");
  assert.ok(tail);
  assert.equal(tail.input.messages[0].content[0].text, "one more thing");
});

test("imported hermes runs validate against session/v0", () => {
  const run = buildHermesRun(SESSION, [
    msg("user", { content: "list files", timestamp: 1001 }),
    msg("assistant", { content: "", reasoning: "I'll ls", tool_calls: JSON.stringify([{ id: "c1", function: { name: "bash", arguments: '{"command":"ls"}' } }]), timestamp: 1002 }),
    msg("tool", { content: "README.md", tool_call_id: "c1", timestamp: 1003 }),
    msg("assistant", { content: "There's a README.", token_count: 12, finish_reason: "stop", timestamp: 1004 }),
    msg("user", { content: "thanks", timestamp: 1005 }),
  ]);
  assert.deepEqual(validateRun(run), []);
});
