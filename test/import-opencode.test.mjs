import assert from "node:assert/strict";
import { test } from "node:test";
import { buildOpencodeRun } from "../cli/import-opencode.mjs";
import { validateRun } from "@session-link/format/validate";

const SESSION = {
  id: "ses_1",
  title: "Fix the bug",
  directory: "/tmp/p",
  model: '{"id":"gpt-5.6-terra-fast","providerID":"openai","variant":"default"}',
  agent: "build",
  version: "1.18.3",
  time_created: 1000,
};
const userMsg = (t, text) => ({ id: "mu", time_created: t, data: { role: "user", time: { created: t } }, parts: [{ type: "text", text }] });
const asstMsg = (id, created, completed, parts, extra = {}) => ({
  id,
  time_created: created,
  data: {
    role: "assistant",
    modelID: "gpt-5.6-terra-fast",
    providerID: "openai",
    tokens: { total: 100, input: 80, output: 20, reasoning: 5, cache: { read: 10, write: 0 } },
    time: { created, completed },
    finish: "stop",
    ...extra,
  },
  parts,
});

test("buildOpencodeRun: basic turn — provenance, model, usage, name from title", () => {
  const run = buildOpencodeRun(SESSION, [
    userMsg(1010, "fix it"),
    asstMsg("m2", 1020, 1030, [
      { type: "step-start", snapshot: "x" },
      { type: "text", text: "done" },
      { type: "step-finish", reason: "stop", tokens: {} },
    ]),
  ]);
  assert.equal(run.name, "Fix the bug");
  assert.deepEqual(run.source, {
    kind: "import",
    harness: "opencode",
    label: "session-import@0.1.0",
    fidelity: "reconstructed",
  });
  assert.equal(run.metadata.session_id, "ses_1");
  assert.equal(run.metadata.cwd, "/tmp/p");
  assert.equal(run.metadata.harness_version, "1.18.3");
  const llm = run.spans.find((s) => s.type === "llm_call");
  assert.deepEqual(llm.model, { id: "gpt-5.6-terra-fast", provider: "openai" });
  assert.deepEqual(llm.usage, { input_tokens: 80, output_tokens: 20, reasoning_tokens: 5, cache_read_tokens: 10 });
  assert.equal(llm.input.messages[0].content[0].text, "fix it");
  assert.equal(llm.output.messages[0].content[0].text, "done");
  assert.equal(llm.started_at, new Date(1020).toISOString());
  assert.equal(llm.ended_at, new Date(1030).toISOString());
});

test("buildOpencodeRun: a tool part becomes a tool_call child with call + result", () => {
  const run = buildOpencodeRun(SESSION, [
    userMsg(1010, "list files"),
    asstMsg("m2", 1020, 1030, [
      { type: "step-start", snapshot: "x" },
      {
        type: "tool",
        tool: "bash",
        callID: "call_1",
        state: { status: "completed", input: { command: "ls -a" }, output: "a\nb\n", time: { start: 1021, end: 1022 } },
      },
      { type: "text", text: "here they are" },
    ]),
  ]);
  const llm = run.spans.find((s) => s.type === "llm_call");
  const tool = run.spans.find((s) => s.type === "tool_call");
  assert.equal(tool.name, "bash");
  assert.equal(tool.parent_id, llm.id);
  assert.deepEqual(tool.input.arguments, { command: "ls -a" });
  assert.equal(tool.input.tool_call_id, "call_1");
  assert.equal(tool.output.result, "a\nb\n");
  assert.equal(tool.status, "ok");
  assert.equal(tool.ended_at, new Date(1022).toISOString());
  // the tool call also appears as a content part in the assistant output
  const toolPart = llm.output.messages[0].content.find((c) => c.type === "tool_call");
  assert.equal(toolPart.id, "call_1");
});

test("buildOpencodeRun: errored tool state marks the tool span", () => {
  const run = buildOpencodeRun(SESSION, [
    userMsg(1010, "go"),
    asstMsg("m2", 1020, 1030, [
      { type: "tool", tool: "bash", callID: "c1", state: { status: "error", error: "boom", input: {}, time: { start: 1021, end: 1022 } } },
    ]),
  ]);
  const tool = run.spans.find((s) => s.type === "tool_call");
  assert.equal(tool.status, "error");
  assert.equal(tool.output.is_error, true);
  assert.equal(tool.output.result, "boom");
});

test("buildOpencodeRun: reasoning part maps to thinking; falls back to session.model", () => {
  const run = buildOpencodeRun(SESSION, [
    userMsg(1010, "think"),
    {
      id: "m2",
      time_created: 1020,
      data: { role: "assistant", time: { created: 1020, completed: 1030 }, finish: "stop" }, // no modelID
      parts: [{ type: "reasoning", text: "let me think" }, { type: "text", text: "answer" }],
    },
  ]);
  const llm = run.spans.find((s) => s.type === "llm_call");
  assert.deepEqual(llm.model, { id: "gpt-5.6-terra-fast", provider: "openai" }); // from session.model
  assert.deepEqual(llm.output.messages[0].content[0], { type: "thinking", text: "let me think" });
});

test("buildOpencodeRun: finish=error marks the llm_call span", () => {
  const run = buildOpencodeRun(SESSION, [
    userMsg(1010, "go"),
    asstMsg("m2", 1020, 1030, [{ type: "text", text: "" }], { finish: "error", error: { message: "rate limited" } }),
  ]);
  const llm = run.spans.find((s) => s.type === "llm_call");
  assert.equal(llm.status, "error");
  assert.deepEqual(llm.error, { message: "rate limited" });
});

test("buildOpencodeRun: a trailing user message survives as a custom span", () => {
  const run = buildOpencodeRun(SESSION, [userMsg(1010, "one more thing")]);
  const tail = run.spans.find((s) => s.type === "custom");
  assert.ok(tail);
  assert.equal(tail.input.messages[0].content[0].text, "one more thing");
});

test("imported opencode runs validate against session/v0", () => {
  const run = buildOpencodeRun(SESSION, [
    userMsg(1010, "list files"),
    asstMsg("m2", 1020, 1035, [
      { type: "step-start", snapshot: "x" },
      { type: "reasoning", text: "I'll run ls" },
      { type: "tool", tool: "bash", callID: "c1", state: { status: "completed", input: { command: "ls" }, output: "README.md", time: { start: 1021, end: 1022 } } },
      { type: "text", text: "There's a README." },
      { type: "step-finish", reason: "stop", tokens: {} },
    ]),
    userMsg(1040, "thanks"),
  ]);
  assert.deepEqual(validateRun(run), []);
});
