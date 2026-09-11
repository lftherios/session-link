import type { ContentPart, Message, Run, Span } from "@session-link/format";

export type FlowBlock =
  | { key: string; kind: "boundary"; spanId: string; scope: string; label: string }
  | { key: string; kind: "msg"; spanId: string; scope: string; msg: Message; err?: string; unitPrefix: string; partIndices?: number[]; unitPrefixes?: string[] };
export type MessageBlock = Extract<FlowBlock, { kind: "msg" }>;
export type Exchange = { id: string; scope: string; child: boolean; agent?: string; prompts: MessageBlock[]; blocks: MessageBlock[] };

const canonical = (v: unknown): string => JSON.stringify(v, (_, value) => value && typeof value === "object" && !Array.isArray(value)
  ? Object.fromEntries(Object.entries(value).sort(([a], [b]) => a.localeCompare(b))) : value);
const data = (s: Span, side: "input" | "output") => (s as unknown as Record<string, Record<string, unknown>>)[side] ?? {};
export const messageText = (msg: Message) => msg.content.filter(p => p.type === "text").map(p => (p as { text: string }).text).join("\n\n");
export const reasoningUnavailable = (part: ContentPart, run?: Run) => part.type === "thinking" &&
  (part.unavailable === true || !String(part.text ?? "").trim() ||
    (run?.source?.kind === "import" && (run.source as { harness?: string }).harness === "codex" && String(part.text).trim() === "[reasoning]"));

// The source indices also address selectable units in the Go excerpt catalog.
// Keep complete-history deduplication scoped to the nearest agent; imports
// and pi SDK captures contain deltas, including intentional repeated prompts.
export function buildFlow(run: Run): FlowBlock[] {
  const byId = new Map(run.spans.map(s => [s.id, s]));
  const scopeOf = (s: Span): string => {
    let p = s.parent_id; const seen = new Set<string>();
    while (p && !seen.has(p)) {
      seen.add(p); const parent = byId.get(p);
      if (parent?.type === "agent") return p;
      p = parent?.parent_id;
    }
    return "";
  };
  const blocks: FlowBlock[] = [], histories = new Map<string, Message[]>(), tools = new Set<string>();
  const delta = run.source?.kind === "import" || (run.source?.kind === "sdk" && run.source.label?.startsWith("pi-extension@"));
  run.spans.forEach((s, si) => {
    const scope = scopeOf(s);
    if (s.type === "agent" && s.parent_id && byId.has(s.parent_id)) {
      blocks.push({ key: `b:${s.id}`, kind: "boundary", spanId: s.id, scope: s.id, label: s.name ?? s.id });
      return;
    }
    const input = data(s, "input"), output = data(s, "output");
    const ins = (input.messages ?? []) as Message[], outs = (output.messages ?? []) as Message[];
    const error = s.status === "error" ? String(s.error?.message ?? "Recorded error") : undefined;
    const push = (msg: Message, prefix: string) => {
      // A tool can occur in both its own span and a subsequent model input.
      // Match the complete result, preserving repeated parts inside a result.
      const partIndices: number[] = [];
      const content = msg.content.filter((p, index) => {
        if (p.type === "tool_call" || p.type === "tool_result") {
          const id = p.type === "tool_call" ? p.id : p.tool_call_id;
          if (id) {
            const key = canonical([scope, p.type, id, p.type === "tool_call" ? [p.name, p.arguments] : p.content]);
            if (tools.has(key)) return false;
            tools.add(key);
          }
        }
        partIndices.push(index); return true;
      });
      if (content.length) blocks.push({ key: prefix, kind: "msg", spanId: s.id, scope, msg: { ...msg, content }, unitPrefix: prefix, partIndices });
    };
    const previous = histories.get(scope) ?? [];
    const replay = !delta && previous.length > 0 && ins.length >= previous.length && previous.every((m, i) => canonical(m) === canonical(ins[i]));
    ins.forEach((m, i) => { if (!replay || i >= previous.length) push(m, `u${si}-in-${i}`); });
    outs.forEach((m, i) => push(m, `u${si}-out-${i}`));
    if (ins.length + outs.length) histories.set(scope, [...ins, ...outs]);
    if (s.type === "tool_call") {
      const id = String(input.tool_call_id ?? "");
      if (input.arguments != null) push({ role: "assistant", content: [{ type: "tool_call", id, name: String(input.name ?? s.name ?? "Tool"), arguments: input.arguments }] } as Message, `u${si}-call`);
      if (output.result != null) {
        const result = output.result;
        const content = Array.isArray(result) && result[0]?.type ? result : [{ type: "text", text: typeof result === "string" ? result : JSON.stringify(result, null, 2) }];
        push({ role: "tool", content: [{ type: "tool_result", tool_call_id: id, content, is_error: s.status === "error" }] } as Message, `u${si}-result`);
      }
    }
    if (error) blocks.push({ key: `u${si}-error`, kind: "msg", spanId: s.id, scope, msg: { role: "system", content: [] }, err: error, unitPrefix: `u${si}-error` });
  });
  return blocks;
}

// Some harnesses put reasoning, tools and answer text in the same message.
// Separate their reader presentations while retaining original part addresses
// for selection, search and exports. The captured message stays untouched.
export function readerFlow(flow: FlowBlock[]): FlowBlock[] {
  return flow.flatMap(block => {
    if (block.kind !== "msg" || block.msg.role !== "assistant" || !messageText(block.msg).trim()) return [block];
    const activity = (type: string) => type === "thinking" || type === "tool_call" || type === "tool_result";
    if (!block.msg.content.some(part => activity(part.type))) return [block];
    const slice = (isActivity: boolean): MessageBlock => {
      const indices = block.msg.content.map((_, index) => index).filter(index => activity(block.msg.content[index].type) === isActivity);
      return { ...block, key: block.key + (isActivity ? "-activity" : ""),
        msg: { ...block.msg, content: indices.map(index => block.msg.content[index]) },
        partIndices: indices.map(index => block.partIndices?.[index] ?? index),
        unitPrefixes: indices.map(index => `${block.unitPrefix}-${block.partIndices?.[index] ?? index}`) };
    };
    return [slice(true), slice(false)];
  });
}

export const selectionPrefixes = (block: MessageBlock) => block.unitPrefixes ?? [block.unitPrefix];

export function exchangesFor(run: Run, flow = buildFlow(run)): Exchange[] {
  const agents = new Map(run.spans.filter(s => s.type === "agent").map(s => [s.id, s]));
  const current = new Map<string, Exchange>(), exchanges: Exchange[] = [];
  for (const block of flow) {
    if (block.kind !== "msg") continue;
    let exchange = current.get(block.scope);
    const user = block.msg.role === "user";
    const adjacentPrompt = user && exchange?.blocks.length === 0 && exchange.prompts.at(-1)?.spanId === block.spanId;
    if (!exchange || (user && !adjacentPrompt)) {
      const agent = agents.get(block.scope);
      exchange = { id: block.key, scope: block.scope, child: !!agent?.parent_id, agent: agent?.name, prompts: [], blocks: [] };
      current.set(block.scope, exchange); exchanges.push(exchange);
    }
    if (user) exchange.prompts.push(block); else exchange.blocks.push(block);
  }
  // System-only setup is accessible in the trace; it isn't an empty exchange.
  return exchanges.filter(e => e.prompts.length || e.blocks.some(b => b.msg.role !== "system" || b.err));
}

export const responseFor = (exchange: Exchange) => exchange.blocks.filter(b => b.msg.role === "assistant" && messageText(b.msg).trim()).at(-1);
export const defaultExchange = (exchanges: Exchange[]) => exchanges.filter(e => !e.child).at(-1) ?? exchanges.at(-1);
export const promptLabel = (e: Exchange) => e.prompts.map(p => messageText(p.msg)).filter(t => t.trim() && !/^\s*<(?:environment_context|system|instructions|AGENTS)/i.test(t)).at(-1)?.trim() || "Recorded exchange";
export const shortText = (text: string, max = 86) => { const clean = text.replace(/\s+/g, " ").trim(); return clean.length > max ? clean.slice(0, max - 1).trimEnd() + "…" : clean; };
export const meaningfulTitle = (name?: string) => !!name?.trim() && !/^(?:untitled(?: session)?|session|[a-f\d-]{24,}|rollout-.*|.*\.(?:jsonl?|spool))$/i.test(name.trim()) && !name.trim().startsWith("/");
export function sessionTitle(run: Run, exchanges: Exchange[]): string {
  const name = run.name?.trim();
  if (name && meaningfulTitle(name)) return name;
  const prompt = exchanges.filter(e => !e.child).map(promptLabel).find(t => t !== "Recorded exchange");
  if (prompt) return shortText(prompt, 90);
  const harness = (run.source as { harness?: string } | undefined)?.harness ?? run.source?.kind ?? "Session";
  return `${harness} · ${run.created_at.slice(0, 10)}`;
}

// Only a fingerprint and reading coordinates enter browser storage, never
// captured text. Local previews use their full content-addressed source ID.
export function readingKey(run: Run, exchanges: Exchange[]): string {
  let hash = 2166136261;
  for (const text of [run.created_at, run.name ?? "", ...exchanges.flatMap(e => [e.id, ...e.prompts.map(p => messageText(p.msg)), ...e.blocks.map(b => messageText(b.msg))])]) {
    for (let i = 0; i < text.length; i++) hash = Math.imul(hash ^ text.charCodeAt(i), 16777619);
  }
  return (hash >>> 0).toString(16);
}
