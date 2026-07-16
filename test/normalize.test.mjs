import assert from "node:assert/strict";
import { test } from "node:test";
import {
  assembleAnthropicSse,
  assembleOpenaiChatSse,
  assembleResponsesSse,
  buildLlmSpan,
  parseSseText,
} from "../cli/normalize.mjs";

test("parseSseText: data lines only, skips [DONE] and non-JSON", () => {
  const text = [
    "event: message_start",
    'data: {"a":1}',
    "data: not json",
    "",
    'data: {"b":2}',
    "data: [DONE]",
    "",
  ].join("\r\n");
  assert.deepEqual(parseSseText(text), [{ a: 1 }, { b: 2 }]);
});

test("assembleAnthropicSse: text, thinking, tool_use json, usage merge", () => {
  const msg = assembleAnthropicSse([
    {
      type: "message_start",
      message: { id: "msg_1", model: "claude-test", role: "assistant", usage: { input_tokens: 10 } },
    },
    { type: "content_block_start", index: 0, content_block: { type: "thinking", thinking: "" } },
    { type: "content_block_delta", index: 0, delta: { type: "thinking_delta", thinking: "hm" } },
    { type: "content_block_delta", index: 0, delta: { type: "signature_delta", signature: "sig" } },
    { type: "content_block_start", index: 1, content_block: { type: "text", text: "" } },
    { type: "content_block_delta", index: 1, delta: { type: "text_delta", text: "Hello" } },
    { type: "content_block_delta", index: 1, delta: { type: "text_delta", text: " world" } },
    { type: "content_block_start", index: 2, content_block: { type: "tool_use", id: "tu_1", name: "calc" } },
    { type: "content_block_delta", index: 2, delta: { type: "input_json_delta", partial_json: '{"a":' } },
    { type: "content_block_delta", index: 2, delta: { type: "input_json_delta", partial_json: "1}" } },
    { type: "message_delta", delta: { stop_reason: "tool_use" }, usage: { output_tokens: 7 } },
  ]);
  assert.equal(msg.content[0].thinking, "hm");
  assert.equal(msg.content[0].signature, "sig");
  assert.equal(msg.content[1].text, "Hello world");
  assert.deepEqual(msg.content[2].input, { a: 1 });
  assert.equal(msg.content[2]._json, undefined);
  assert.equal(msg.stop_reason, "tool_use");
  assert.deepEqual(msg.usage, { input_tokens: 10, output_tokens: 7 });
});

test("assembleAnthropicSse: no message_start → null", () => {
  assert.equal(assembleAnthropicSse([]), null);
  assert.equal(assembleAnthropicSse([{ type: "ping" }]), null);
});

test("assembleOpenaiChatSse: indexed tool_call accumulation and usage", () => {
  const out = assembleOpenaiChatSse([
    { id: "cc_1", model: "gpt-test", created: 1, choices: [{ index: 0, delta: { role: "assistant" } }] },
    { choices: [{ index: 0, delta: { tool_calls: [{ index: 0, id: "call_1", function: { name: "get_", arguments: "" } }] } }] },
    { choices: [{ index: 0, delta: { tool_calls: [{ index: 0, function: { name: "weather", arguments: '{"c":' } }] } }] },
    { choices: [{ index: 0, delta: { tool_calls: [{ index: 0, function: { arguments: '"ams"}' } }] } }] },
    { choices: [{ index: 0, delta: {}, finish_reason: "tool_calls" }] },
    { choices: [], usage: { prompt_tokens: 5, completion_tokens: 3 } },
  ]);
  const m = out.choices[0].message;
  assert.equal(out.id, "cc_1");
  assert.equal(m.content, null); // tool-call turn: content must be null, not ""
  assert.deepEqual(m.tool_calls, [
    { id: "call_1", type: "function", function: { name: "get_weather", arguments: '{"c":"ams"}' } },
  ]);
  assert.equal(out.choices[0].finish_reason, "tool_calls");
  assert.deepEqual(out.usage, { prompt_tokens: 5, completion_tokens: 3 });
});

test("assembleOpenaiChatSse: plain text stream", () => {
  const out = assembleOpenaiChatSse([
    { id: "cc_2", choices: [{ index: 0, delta: { role: "assistant", content: "Hel" } }] },
    { choices: [{ index: 0, delta: { content: "lo" }, finish_reason: "stop" }] },
  ]);
  assert.equal(out.choices[0].message.content, "Hello");
  assert.equal(out.choices[0].finish_reason, "stop");
  assert.equal(assembleOpenaiChatSse([]), null);
});

test("assembleResponsesSse: extracts the response.completed payload", () => {
  const response = { id: "resp_1", output: [] };
  assert.equal(
    assembleResponsesSse([{ type: "response.created" }, { type: "response.completed", response }]),
    response,
  );
  assert.equal(assembleResponsesSse([{ type: "response.created" }]), null);
});

test("buildLlmSpan anthropic: ok path maps system, tools, usage, output", () => {
  const span = buildLlmSpan("anthropic.messages", {
    id: "s1",
    parent_id: "root",
    request: {
      model: "claude-x",
      system: [{ text: "a" }, { text: "b" }],
      messages: [{ role: "user", content: "hi" }],
      tools: [{ name: "t1", input_schema: {} }, { not_a_tool: true }],
      temperature: 0.5,
    },
    response: {
      content: [{ type: "text", text: "yo" }],
      usage: { input_tokens: 1, output_tokens: 2, cache_read_input_tokens: 3 },
    },
    started_at: "2026-07-16T10:00:00.000Z",
    ended_at: "2026-07-16T10:00:01.000Z",
    httpStatus: 200,
    streamed: true,
  });
  assert.equal(span.status, "ok");
  assert.equal(span.input.system, "a\n\nb");
  assert.deepEqual(span.input.messages[0].content, [{ type: "text", text: "hi" }]);
  assert.equal(span.input.tools.length, 1); // malformed tool dropped from normalized view
  assert.equal(span.params.temperature, 0.5);
  assert.deepEqual(span.usage, { input_tokens: 1, output_tokens: 2, cache_read_tokens: 3 });
  assert.equal(span.output.messages[0].content[0].text, "yo");
  assert.equal(span.metadata.stream, true);
  assert.equal(span.raw.request.model, "claude-x"); // raw is sacred
});

test("buildLlmSpan: transport error and http error become error spans", () => {
  const transport = buildLlmSpan("anthropic.messages", {
    id: "s1",
    parent_id: "root",
    request: null,
    response: null,
    started_at: "2026-07-16T10:00:00.000Z",
    ended_at: "2026-07-16T10:00:01.000Z",
    httpStatus: 502,
    transportError: "upstream unreachable: boom",
  });
  assert.equal(transport.status, "error");
  assert.equal(transport.error.message, "upstream unreachable: boom");
  assert.equal(transport.model.id, "unknown");
  assert.deepEqual(transport.output.messages, []);

  const http429 = buildLlmSpan("openai.chat", {
    id: "s2",
    parent_id: "root",
    request: { model: "gpt-test" },
    response: { error: { message: "rate limited" } },
    started_at: "2026-07-16T10:00:00.000Z",
    ended_at: "2026-07-16T10:00:01.000Z",
    httpStatus: 429,
  });
  assert.equal(http429.status, "error");
  assert.equal(http429.error.message, "rate limited");
});

test("buildLlmSpan: capture gap marks fidelity partial", () => {
  const span = buildLlmSpan("openai.chat", {
    id: "s1",
    parent_id: "root",
    request: { model: "gpt-test" },
    response: { unassembled_sse: "data: garbage" },
    started_at: "2026-07-16T10:00:00.000Z",
    ended_at: "2026-07-16T10:00:01.000Z",
    httpStatus: 200,
    captureGap: true,
  });
  assert.equal(span.fidelity, "partial");
  assert.equal(span.status, "ok");
});

test("buildLlmSpan openai.chat: tool message and tool_calls round-trip", () => {
  const span = buildLlmSpan("openai.chat", {
    id: "s1",
    parent_id: "root",
    request: {
      model: "gpt-test",
      messages: [
        { role: "user", content: "weather?" },
        { role: "tool", tool_call_id: "call_1", content: "42" },
      ],
    },
    response: {
      choices: [
        {
          message: {
            role: "assistant",
            content: null,
            tool_calls: [{ id: "call_2", function: { name: "f", arguments: '{"x":1}' } }],
          },
        },
      ],
    },
    started_at: "2026-07-16T10:00:00.000Z",
    ended_at: "2026-07-16T10:00:01.000Z",
    httpStatus: 200,
  });
  const toolMsg = span.input.messages[1];
  assert.equal(toolMsg.content[0].type, "tool_result");
  assert.equal(toolMsg.content[0].tool_call_id, "call_1");
  const outPart = span.output.messages[0].content[0];
  assert.deepEqual(outPart, { type: "tool_call", id: "call_2", name: "f", arguments: { x: 1 } });
});
