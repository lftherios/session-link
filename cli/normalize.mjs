/**
 * Provider wire formats → session/v0 llm_call spans.
 *
 * Normalization is best-effort and lossy by design — the verbatim payloads
 * ride along in span.raw ("raw is sacred"). Unknown content block types
 * pass through untouched: the wire format is open-world and the viewer
 * renders a JSON fallback. Anything that would produce a schema-invalid
 * part (e.g. a tool_use block missing its id) degrades to a data part
 * instead, so captures always validate.
 */

const PARAM_KEYS = [
  "temperature",
  "max_tokens",
  "max_completion_tokens",
  "max_output_tokens",
  "top_p",
  "top_k",
  "stop",
  "stop_sequences",
  "reasoning_effort",
  "seed",
];

function pickParams(body) {
  const out = {};
  for (const k of PARAM_KEYS) if (body[k] !== undefined) out[k] = body[k];
  return Object.keys(out).length ? out : undefined;
}

function tryJson(text) {
  if (typeof text !== "string") return text;
  try {
    return JSON.parse(text);
  } catch {
    return text;
  }
}

const dataPart = (data) => ({ type: "data", data });

/* ------------------------------------------------- anthropic /v1/messages */

function anthropicPart(block) {
  if (!block || typeof block.type !== "string") return dataPart(block);
  switch (block.type) {
    case "text":
      return { type: "text", text: block.text ?? "" };
    case "thinking":
      return { type: "thinking", text: block.thinking ?? "" };
    case "tool_use":
      if (typeof block.id !== "string" || typeof block.name !== "string")
        return dataPart(block);
      return { type: "tool_call", id: block.id, name: block.name, arguments: block.input };
    case "tool_result":
      if (typeof block.tool_use_id !== "string") return dataPart(block);
      return {
        type: "tool_result",
        tool_call_id: block.tool_use_id,
        ...(block.is_error !== undefined ? { is_error: block.is_error } : {}),
        content: anthropicContent(block.content),
      };
    case "image":
      if (block.source?.type === "url")
        return { type: "image", url: block.source.url, mime: block.source.media_type };
      // base64 bytes stay in raw; don't balloon the normalized view
      return dataPart({
        kind: "image",
        media_type: block.source?.media_type,
        note: "inline bytes omitted; see raw",
      });
    default:
      return block; // open-world pass-through
  }
}

export function anthropicContent(content) {
  if (content == null) return [];
  if (typeof content === "string") return [{ type: "text", text: content }];
  return content.map(anthropicPart);
}

function anthropicSystem(system) {
  if (system == null) return undefined;
  if (typeof system === "string") return system;
  return system.map((b) => b?.text ?? "").join("\n\n");
}

export function anthropicUsage(usage) {
  if (!usage) return undefined;
  const out = {};
  if (usage.input_tokens != null) out.input_tokens = usage.input_tokens;
  if (usage.output_tokens != null) out.output_tokens = usage.output_tokens;
  if (usage.cache_read_input_tokens != null)
    out.cache_read_tokens = usage.cache_read_input_tokens;
  return Object.keys(out).length ? out : undefined;
}

/* -------------------------------------------- openai /v1/chat/completions */

function openaiChatParts(m) {
  const parts = [];
  if (typeof m.content === "string" && m.content) {
    parts.push({ type: "text", text: m.content });
  } else if (Array.isArray(m.content)) {
    for (const p of m.content) {
      if (!p || typeof p.type !== "string") parts.push(dataPart(p));
      else if (p.type === "text") parts.push({ type: "text", text: p.text ?? "" });
      else if (p.type === "image_url" && p.image_url?.url)
        parts.push({ type: "image", url: p.image_url.url });
      else parts.push(p); // open-world pass-through
    }
  }
  for (const tc of m.tool_calls ?? []) {
    if (typeof tc?.id === "string" && typeof tc?.function?.name === "string") {
      parts.push({
        type: "tool_call",
        id: tc.id,
        name: tc.function.name,
        arguments: tryJson(tc.function.arguments),
      });
    } else {
      parts.push(dataPart(tc));
    }
  }
  return parts;
}

function openaiChatMessages(messages = []) {
  return messages.map((m) => {
    if (m.role === "tool" && typeof m.tool_call_id === "string") {
      return {
        role: "tool",
        content: [
          {
            type: "tool_result",
            tool_call_id: m.tool_call_id,
            content:
              typeof m.content === "string"
                ? [{ type: "text", text: m.content }]
                : openaiChatParts(m),
          },
        ],
      };
    }
    return { role: m.role ?? "user", content: openaiChatParts(m) };
  });
}

function openaiTools(tools) {
  if (!Array.isArray(tools)) return undefined;
  const out = tools
    .map((t) => t?.function ?? t)
    .filter((f) => typeof f?.name === "string")
    .map((f) => ({
      name: f.name,
      ...(f.description ? { description: f.description } : {}),
      ...(f.parameters ? { input_schema: f.parameters } : {}),
    }));
  return out.length ? out : undefined;
}

function openaiUsage(u) {
  if (!u) return undefined;
  const out = {};
  const input = u.input_tokens ?? u.prompt_tokens;
  const output = u.output_tokens ?? u.completion_tokens;
  const cached =
    u.input_tokens_details?.cached_tokens ?? u.prompt_tokens_details?.cached_tokens;
  if (input != null) out.input_tokens = input;
  if (output != null) out.output_tokens = output;
  if (cached != null) out.cache_read_tokens = cached;
  return Object.keys(out).length ? out : undefined;
}

/* --------------------------------------------------- openai /v1/responses */

function responsesContent(content) {
  if (typeof content === "string") return [{ type: "text", text: content }];
  if (!Array.isArray(content)) return [];
  return content.map((p) => {
    if (!p || typeof p.type !== "string") return dataPart(p);
    if (p.type === "input_text" || p.type === "output_text")
      return { type: "text", text: p.text ?? "" };
    if (p.type === "input_image" && p.image_url)
      return { type: "image", url: p.image_url };
    return p; // open-world pass-through
  });
}

function responsesInputMessages(input) {
  if (typeof input === "string")
    return [{ role: "user", content: [{ type: "text", text: input }] }];
  if (!Array.isArray(input)) return [];
  const messages = [];
  for (const item of input) {
    if (item?.role) {
      messages.push({ role: item.role, content: responsesContent(item.content) });
    } else if (item?.type === "function_call" && typeof item.call_id === "string") {
      messages.push({
        role: "assistant",
        content: [
          {
            type: "tool_call",
            id: item.call_id,
            name: item.name ?? "function",
            arguments: tryJson(item.arguments),
          },
        ],
      });
    } else if (item?.type === "function_call_output" && typeof item.call_id === "string") {
      messages.push({
        role: "tool",
        content: [
          {
            type: "tool_result",
            tool_call_id: item.call_id,
            content: [{ type: "text", text: String(item.output ?? "") }],
          },
        ],
      });
    } else {
      messages.push({ role: "user", content: [dataPart(item)] });
    }
  }
  return messages;
}

function responsesOutputMessages(output) {
  if (!Array.isArray(output)) return [];
  const messages = [];
  for (const item of output) {
    if (item?.type === "message") {
      messages.push({
        role: item.role ?? "assistant",
        content: responsesContent(item.content),
      });
    } else if (item?.type === "function_call" && typeof item.call_id === "string") {
      messages.push({
        role: "assistant",
        content: [
          {
            type: "tool_call",
            id: item.call_id,
            name: item.name ?? "function",
            arguments: tryJson(item.arguments),
          },
        ],
      });
    } else if (item?.type === "reasoning") {
      const text = (item.summary ?? []).map((s) => s?.text ?? "").join("\n");
      messages.push({
        role: "assistant",
        content: [{ type: "thinking", text }],
      });
    } else {
      messages.push({ role: "assistant", content: [dataPart(item)] });
    }
  }
  return messages;
}

/* ----------------------------------------------------------- SSE handling */

export function parseSseText(text) {
  const events = [];
  for (const line of text.split(/\r?\n/)) {
    if (!line.startsWith("data:")) continue;
    const payload = line.slice(5).trim();
    if (!payload || payload === "[DONE]") continue;
    try {
      events.push(JSON.parse(payload));
    } catch {
      /* non-JSON data line */
    }
  }
  return events;
}

/** Reassemble the final Anthropic message object from a messages stream. */
export function assembleAnthropicSse(events) {
  let message = null;
  const blocks = [];
  for (const ev of events) {
    switch (ev.type) {
      case "message_start":
        message = structuredClone(ev.message ?? {});
        break;
      case "content_block_start":
        blocks[ev.index] = structuredClone(ev.content_block ?? {});
        break;
      case "content_block_delta": {
        const b = blocks[ev.index];
        const d = ev.delta;
        if (!b || !d) break;
        if (d.type === "text_delta") b.text = (b.text ?? "") + d.text;
        else if (d.type === "thinking_delta") b.thinking = (b.thinking ?? "") + d.thinking;
        else if (d.type === "input_json_delta") b._json = (b._json ?? "") + d.partial_json;
        else if (d.type === "signature_delta") b.signature = (b.signature ?? "") + d.signature;
        break;
      }
      case "message_delta":
        if (message) {
          Object.assign(message, ev.delta ?? {});
          if (ev.usage) message.usage = { ...message.usage, ...ev.usage };
        }
        break;
    }
  }
  if (!message) return null;
  message.content = blocks.filter(Boolean).map((b) => {
    if (b._json !== undefined) {
      try {
        b.input = b._json ? JSON.parse(b._json) : {};
      } catch {
        /* keep partial json out of input */
      }
      delete b._json;
    }
    return b;
  });
  return message;
}

/** Reassemble a chat.completion object from a chat/completions stream. */
export function assembleOpenaiChatSse(events) {
  let base = null;
  let role = "assistant";
  let content = "";
  const toolCalls = [];
  let finish = null;
  let usage = null;
  for (const ev of events) {
    if (ev.usage) usage = ev.usage;
    if (!base && ev.id) base = { id: ev.id, model: ev.model, created: ev.created };
    const choice = ev.choices?.[0];
    if (!choice) continue;
    const d = choice.delta ?? {};
    if (d.role) role = d.role;
    if (typeof d.content === "string") content += d.content;
    for (const tc of d.tool_calls ?? []) {
      const slot = (toolCalls[tc.index ?? 0] ??= {
        id: "",
        type: "function",
        function: { name: "", arguments: "" },
      });
      if (tc.id) slot.id = tc.id;
      if (tc.function?.name) slot.function.name += tc.function.name;
      if (tc.function?.arguments) slot.function.arguments += tc.function.arguments;
    }
    if (choice.finish_reason) finish = choice.finish_reason;
  }
  if (!base && !content && !toolCalls.length) return null;
  const calls = toolCalls.filter(Boolean);
  return {
    ...(base ?? {}),
    object: "chat.completion",
    choices: [
      {
        index: 0,
        message: {
          role,
          content: content || (calls.length ? null : ""),
          ...(calls.length ? { tool_calls: calls } : {}),
        },
        finish_reason: finish,
      },
    ],
    ...(usage ? { usage } : {}),
  };
}

/** The responses stream carries the whole final object in response.completed. */
export function assembleResponsesSse(events) {
  for (let i = events.length - 1; i >= 0; i--) {
    if (events[i]?.type === "response.completed" && events[i].response)
      return events[i].response;
  }
  return null;
}

/* ------------------------------------------------------------ the builder */

const SPAN_NAMES = {
  "anthropic.messages": "messages.create",
  "openai.chat": "chat.completions",
  "openai.responses": "responses.create",
};

export function buildLlmSpan(kind, ctx) {
  const {
    id,
    parent_id,
    request,
    response,
    started_at,
    ended_at,
    httpStatus,
    streamed,
    captureGap,
    transportError,
  } = ctx;
  const req = request ?? {};
  const ok = !transportError && httpStatus >= 200 && httpStatus < 300;
  const span = {
    id,
    parent_id,
    type: "llm_call",
    name: SPAN_NAMES[kind],
    started_at,
    ended_at,
    status: ok ? "ok" : "error",
    model: {
      id: String(req.model ?? "unknown"),
      provider: kind === "anthropic.messages" ? "anthropic" : "openai",
    },
    input: { messages: [] },
    output: { messages: [] },
    raw: { request: request ?? null, response: response ?? null },
  };
  const params = pickParams(req);
  if (params) span.params = params;
  if (streamed) span.metadata = { stream: true };
  if (captureGap) span.fidelity = "partial";
  if (!ok) {
    span.error = {
      message:
        transportError ?? response?.error?.message ?? `HTTP ${httpStatus}`,
    };
  }

  if (kind === "anthropic.messages") {
    const system = anthropicSystem(req.system);
    if (system) span.input.system = system;
    span.input.messages = (req.messages ?? []).map((m) => ({
      role: m.role ?? "user",
      content: anthropicContent(m.content),
    }));
    const tools = (req.tools ?? []).filter((t) => typeof t?.name === "string");
    if (tools.length)
      span.input.tools = tools.map((t) => ({
        name: t.name,
        ...(t.description ? { description: t.description } : {}),
        ...(t.input_schema ? { input_schema: t.input_schema } : {}),
      }));
    if (ok && Array.isArray(response?.content))
      span.output.messages = [
        { role: "assistant", content: anthropicContent(response.content) },
      ];
    const usage = anthropicUsage(response?.usage);
    if (usage) span.usage = usage;
  } else if (kind === "openai.chat") {
    span.input.messages = openaiChatMessages(req.messages);
    const tools = openaiTools(req.tools);
    if (tools) span.input.tools = tools;
    const msg = response?.choices?.[0]?.message;
    if (ok && msg) span.output.messages = openaiChatMessages([msg]);
    const usage = openaiUsage(response?.usage);
    if (usage) span.usage = usage;
  } else if (kind === "openai.responses") {
    if (typeof req.instructions === "string" && req.instructions)
      span.input.system = req.instructions;
    span.input.messages = responsesInputMessages(req.input);
    const tools = openaiTools(req.tools);
    if (tools) span.input.tools = tools;
    if (ok) span.output.messages = responsesOutputMessages(response?.output);
    const usage = openaiUsage(response?.usage);
    if (usage) span.usage = usage;
  }

  return span;
}
