import assert from "node:assert/strict";
import { test } from "node:test";
import { transcriptToRun } from "../cli/import-session.mjs";
import { validateRun } from "@session-link/format/validate";

const line = (o) => JSON.stringify(o);
const ts = (n) => `2026-07-10T10:00:${String(n).padStart(2, "0")}.000Z`;

const BASE = { sessionId: "sess-1", cwd: "/tmp/p", version: "1.0.0" };

test("transcriptToRun: basic turn with summary name and usage", () => {
  const run = transcriptToRun([
    line({ type: "summary", summary: "fix the bug" }),
    line({ ...BASE, type: "user", timestamp: ts(0), message: { role: "user", content: "hello" } }),
    line({
      ...BASE,
      type: "assistant",
      timestamp: ts(1),
      message: {
        role: "assistant",
        model: "claude-test",
        content: [{ type: "text", text: "hi!" }],
        usage: { input_tokens: 9, output_tokens: 4, cache_read_input_tokens: 100 },
      },
    }),
  ]);
  assert.equal(run.name, "fix the bug");
  assert.equal(run.source.fidelity, "reconstructed");
  assert.equal(run.metadata.session_id, "sess-1");
  const llm = run.spans.find((s) => s.type === "llm_call");
  assert.equal(llm.model.id, "claude-test");
  assert.equal(llm.input.messages[0].content[0].text, "hello");
  assert.deepEqual(llm.usage, { input_tokens: 9, output_tokens: 4, cache_read_tokens: 100 });
});

test("transcriptToRun: tool_use pairs with tool_result, prefers toolUseResult", () => {
  const run = transcriptToRun([
    line({ ...BASE, type: "user", timestamp: ts(0), message: { role: "user", content: "ls" } }),
    line({
      ...BASE,
      type: "assistant",
      timestamp: ts(1),
      message: {
        role: "assistant",
        model: "claude-test",
        content: [{ type: "tool_use", id: "tu_1", name: "Bash", input: { command: "ls" } }],
      },
    }),
    line({
      ...BASE,
      type: "user",
      timestamp: ts(2),
      toolUseResult: { stdout: "rich result" },
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "tu_1", content: "plain" }],
      },
    }),
    line({
      ...BASE,
      type: "assistant",
      timestamp: ts(3),
      message: { role: "assistant", model: "claude-test", content: [{ type: "text", text: "done" }] },
    }),
  ]);
  const tool = run.spans.find((s) => s.type === "tool_call");
  assert.equal(tool.name, "Bash");
  assert.equal(tool.ended_at, ts(2));
  assert.deepEqual(tool.output.result, { stdout: "rich result" });
  assert.equal(tool.status, "ok");
});

test("transcriptToRun: is_error tool_result marks the tool span", () => {
  const run = transcriptToRun([
    line({ ...BASE, type: "user", timestamp: ts(0), message: { role: "user", content: "go" } }),
    line({
      ...BASE,
      type: "assistant",
      timestamp: ts(1),
      message: {
        role: "assistant",
        model: "claude-test",
        content: [{ type: "tool_use", id: "tu_1", name: "Bash", input: {} }],
      },
    }),
    line({
      ...BASE,
      type: "user",
      timestamp: ts(2),
      message: {
        role: "user",
        content: [{ type: "tool_result", tool_use_id: "tu_1", is_error: true, content: "boom" }],
      },
    }),
    line({
      ...BASE,
      type: "assistant",
      timestamp: ts(3),
      message: { role: "assistant", model: "claude-test", content: [{ type: "text", text: "hm" }] },
    }),
  ]);
  assert.equal(run.spans.find((s) => s.type === "tool_call").status, "error");
});

test("transcriptToRun: sidechains skipped and counted", () => {
  const run = transcriptToRun([
    line({ ...BASE, type: "user", timestamp: ts(0), message: { role: "user", content: "q" } }),
    line({ ...BASE, type: "user", isSidechain: true, timestamp: ts(1), message: { role: "user", content: "sub" } }),
    line({ ...BASE, type: "assistant", isSidechain: true, timestamp: ts(2), message: { role: "assistant", content: [] } }),
    line({
      ...BASE,
      type: "assistant",
      timestamp: ts(3),
      message: { role: "assistant", model: "claude-test", content: [{ type: "text", text: "a" }] },
    }),
  ]);
  assert.equal(run.metadata.sidechain_entries_skipped, 2);
  assert.equal(run.spans.filter((s) => s.type === "llm_call").length, 1);
});

test("transcriptToRun: trailing user message survives as a custom span", () => {
  const run = transcriptToRun([
    line({ ...BASE, type: "user", timestamp: ts(0), message: { role: "user", content: "q" } }),
    line({
      ...BASE,
      type: "assistant",
      timestamp: ts(1),
      message: { role: "assistant", model: "claude-test", content: [{ type: "text", text: "a" }] },
    }),
    line({ ...BASE, type: "user", timestamp: ts(2), message: { role: "user", content: "one more question" } }),
  ]);
  const tail = run.spans.find((s) => s.type === "custom");
  assert.ok(tail, "trailing user message must not be dropped");
  assert.equal(tail.input.messages[0].content[0].text, "one more question");
});

test("transcriptToRun: unparseable and empty input", () => {
  assert.equal(transcriptToRun(["not json", line({ type: "summary", summary: "x" })]), null);
});

test("imported runs validate against session/v0", async () => {
  const run = transcriptToRun([
    line({ ...BASE, type: "user", timestamp: ts(0), message: { role: "user", content: "ls" } }),
    line({
      ...BASE,
      type: "assistant",
      timestamp: ts(1),
      message: {
        role: "assistant",
        model: "claude-test",
        content: [{ type: "tool_use", id: "tu_1", name: "Bash", input: { command: "ls" } }],
      },
    }),
    line({
      ...BASE,
      type: "user",
      timestamp: ts(2),
      message: { role: "user", content: [{ type: "tool_result", tool_use_id: "tu_1", content: "ok" }] },
    }),
    line({ ...BASE, type: "user", timestamp: ts(3), message: { role: "user", content: "trailing" } }),
  ]);
  assert.deepEqual(validateRun(run), []);
});
