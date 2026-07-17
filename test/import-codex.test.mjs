import assert from "node:assert/strict";
import { test } from "node:test";
import { codexRolloutToRun, looksLikeCodex } from "../cli/import-codex.mjs";
import { validateRun } from "@session-link/format/validate";

const ts = (n) => `2026-07-17T10:00:0${n}.000Z`;
const line = (o) => JSON.stringify(o);
const ri = (n, payload) => line({ timestamp: ts(n), type: "response_item", payload });
const meta = () =>
  line({
    timestamp: ts(0),
    type: "session_meta",
    payload: { session_id: "sess-1", cwd: "/tmp/p", model_provider: "openai", base_instructions: { text: "You are Codex." }, timestamp: ts(0), cli_version: "0.144.5" },
  });
const turnCtx = (turn, model) => line({ timestamp: ts(0), type: "turn_context", payload: { turn_id: turn, model } });
const tokens = (n, u) => line({ timestamp: ts(n), type: "event_msg", payload: { type: "token_count", info: { last_token_usage: u } } });
const msg = (n, role, text, turn) => ri(n, { type: "message", role, content: [{ type: role === "assistant" ? "output_text" : "input_text", text }], ...(turn ? { internal_chat_message_metadata_passthrough: { turn_id: turn } } : {}) });

test("codexRolloutToRun: a turn collapses to one llm_call with system, model, usage, tool child", () => {
  const run = codexRolloutToRun([
    meta(),
    turnCtx("t1", "gpt-5.5"),
    msg(1, "developer", "sandbox instructions"),
    msg(2, "user", "Run ls and report", "t1"),
    ri(3, { type: "function_call", name: "exec_command", arguments: '{"cmd":"ls"}', call_id: "call_1", internal_chat_message_metadata_passthrough: { turn_id: "t1" } }),
    ri(4, { type: "function_call_output", call_id: "call_1", output: "a\nb" }),
    tokens(5, { input_tokens: 100, cached_input_tokens: 50, output_tokens: 20, reasoning_output_tokens: 5 }),
    msg(6, "assistant", "Files: a, b", "t1"),
  ]);
  assert.equal(run.name, "Run ls and report");
  assert.deepEqual(run.source, { kind: "import", harness: "codex", label: "session-import@0.1.0", fidelity: "reconstructed" });
  assert.equal(run.metadata.session_id, "sess-1");
  assert.equal(run.metadata.cwd, "/tmp/p");
  assert.equal(run.metadata.harness_version, "0.144.5");
  const llm = run.spans.find((s) => s.type === "llm_call");
  assert.deepEqual(llm.model, { id: "gpt-5.5", provider: "openai" });
  assert.equal(llm.input.system, "You are Codex.");
  assert.equal(llm.input.messages[0].role, "developer");
  assert.equal(llm.input.messages[1].content[0].text, "Run ls and report");
  assert.deepEqual(llm.usage, { input_tokens: 100, output_tokens: 20, cache_read_tokens: 50, reasoning_tokens: 5 });
  const out = llm.output.messages[0].content;
  assert.ok(out.some((c) => c.type === "text" && c.text === "Files: a, b"));
  const tc = out.find((c) => c.type === "tool_call");
  assert.equal(tc.name, "exec_command");
  assert.equal(tc.id, "call_1");
  const tool = run.spans.find((s) => s.type === "tool_call");
  assert.equal(tool.parent_id, llm.id);
  assert.deepEqual(tool.input.arguments, { cmd: "ls" });
  assert.equal(tool.output.result, "a\nb");
  assert.equal(tool.ended_at, ts(4));
});

test("codexRolloutToRun: reasoning items map to thinking", () => {
  const run = codexRolloutToRun([
    meta(),
    turnCtx("t1", "gpt-5.5"),
    msg(1, "user", "think about it", "t1"),
    ri(2, { type: "reasoning", summary: [{ type: "summary_text", text: "let me consider" }] }),
    msg(3, "assistant", "answer", "t1"),
  ]);
  const out = run.spans.find((s) => s.type === "llm_call").output.messages[0].content;
  assert.deepEqual(out[0], { type: "thinking", text: "let me consider" });
});

test("codexRolloutToRun: two turns become two llm_call spans", () => {
  const run = codexRolloutToRun([
    meta(),
    turnCtx("t1", "gpt-5.5"),
    msg(1, "user", "first", "t1"),
    msg(2, "assistant", "one", "t1"),
    turnCtx("t2", "gpt-5.5"),
    msg(3, "user", "second", "t2"),
    msg(4, "assistant", "two", "t2"),
  ]);
  assert.equal(run.spans.filter((s) => s.type === "llm_call").length, 2);
});

test("codexRolloutToRun: trailing user turn with no reply survives as a custom span", () => {
  const run = codexRolloutToRun([meta(), turnCtx("t1", "gpt-5.5"), msg(1, "user", "one more thing", "t1")]);
  const tail = run.spans.find((s) => s.type === "custom");
  assert.ok(tail);
  assert.equal(tail.input.messages[0].content[0].text, "one more thing");
});

test("codexRolloutToRun: empty input returns null; looksLikeCodex recognizes rollouts", () => {
  assert.equal(codexRolloutToRun(["not json"]), null);
  assert.equal(looksLikeCodex([{ type: "session_meta" }]), true);
  assert.equal(looksLikeCodex([{ type: "user" }, { type: "assistant" }]), false);
  assert.equal(looksLikeCodex([{ type: "session" }, { type: "message" }]), false);
});

test("imported codex runs validate against session/v0", () => {
  const run = codexRolloutToRun([
    meta(),
    turnCtx("t1", "gpt-5.5"),
    msg(1, "developer", "sandbox"),
    msg(2, "user", "list files", "t1"),
    ri(3, { type: "reasoning", summary: [{ text: "I'll run ls" }] }),
    ri(3, { type: "function_call", name: "exec_command", arguments: '{"cmd":"ls"}', call_id: "c1" }),
    ri(4, { type: "function_call_output", call_id: "c1", output: "README.md" }),
    tokens(5, { input_tokens: 12, output_tokens: 3 }),
    msg(6, "assistant", "There's a README.", "t1"),
    msg(7, "user", "thanks", "t1"),
  ]);
  assert.deepEqual(validateRun(run), []);
});
