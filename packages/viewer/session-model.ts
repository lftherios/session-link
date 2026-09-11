import type { ContentPart, Message, Run, Span } from "@session-link/format";

export type FlowBlock =
  | { key: string; kind: "boundary"; spanId: string; scope: string; label: string }
  | { key: string; kind: "msg"; spanId: string; scope: string; msg: Message; err?: string; unitPrefix: string; partIndices?: number[]; unitPrefixes?: string[]; at?: string };
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
    // `at` is when the message was recorded: inputs at the span start,
    // outputs and results at its end.
    const push = (msg: Message, prefix: string, at?: string) => {
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
      if (content.length) blocks.push({ key: prefix, kind: "msg", spanId: s.id, scope, msg: { ...msg, content }, unitPrefix: prefix, partIndices, at });
    };
    const previous = histories.get(scope) ?? [];
    const replay = !delta && previous.length > 0 && ins.length >= previous.length && previous.every((m, i) => canonical(m) === canonical(ins[i]));
    ins.forEach((m, i) => { if (!replay || i >= previous.length) push(m, `u${si}-in-${i}`, s.started_at); });
    outs.forEach((m, i) => push(m, `u${si}-out-${i}`, s.ended_at ?? s.started_at));
    if (ins.length + outs.length) histories.set(scope, [...ins, ...outs]);
    if (s.type === "tool_call") {
      const id = String(input.tool_call_id ?? "");
      if (input.arguments != null) push({ role: "assistant", content: [{ type: "tool_call", id, name: String(input.name ?? s.name ?? "Tool"), arguments: input.arguments }] } as Message, `u${si}-call`, s.started_at);
      if (output.result != null) {
        const result = output.result;
        const content = Array.isArray(result) && result[0]?.type ? result : [{ type: "text", text: typeof result === "string" ? result : JSON.stringify(result, null, 2) }];
        push({ role: "tool", content: [{ type: "tool_result", tool_call_id: id, content, is_error: s.status === "error" }] } as Message, `u${si}-result`, s.ended_at ?? s.started_at);
      }
    }
    if (error) blocks.push({ key: `u${si}-error`, kind: "msg", spanId: s.id, scope, msg: { role: "system", content: [] }, err: error, unitPrefix: `u${si}-error`, at: s.ended_at ?? s.started_at });
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

// Harness wrappers carried in user messages: environment and instruction
// blocks, reminders and local slash-command records. They are provided
// context, not human input. Keep in sync with go/internal/share/catalog.go.
const HARNESS_WRAPPER = /^\s*<(?:environment_context|system|instructions|user_instructions|AGENTS|local-command-|command-name|command-message|command-args)/i;
export type ReadingRole = "human" | "context" | "tool" | "agent";
// How the reader treats a message, following the content model. Some
// harnesses record tool results and injected context with the user role.
export function readingRole(msg: Message): ReadingRole {
  if (msg.role === "system" || msg.role === "developer") return "context";
  if (msg.role === "tool") return "tool";
  if (msg.role !== "user") return "agent";
  if (msg.content.length && msg.content.every(part => part.type === "tool_result")) return "tool";
  const texts = msg.content.map(part => part.type === "text" ? part.text : null);
  return texts.length && texts.every(text => text !== null && HARNESS_WRAPPER.test(text)) ? "context" : "human";
}

export function exchangesFor(run: Run, flow = buildFlow(run)): Exchange[] {
  const agents = new Map(run.spans.filter(s => s.type === "agent").map(s => [s.id, s]));
  const current = new Map<string, Exchange>(), held = new Map<string, MessageBlock[]>(), exchanges: Exchange[] = [];
  const open = (block: MessageBlock) => {
    const agent = agents.get(block.scope);
    const exchange: Exchange = { id: block.key, scope: block.scope, child: !!agent?.parent_id, agent: agent?.name, prompts: [], blocks: [] };
    current.set(block.scope, exchange); exchanges.push(exchange);
    return exchange;
  };
  const isContext = (block: MessageBlock) => !block.err && readingRole(block.msg) === "context";
  for (const block of flow) {
    if (block.kind !== "msg") continue;
    // Provided context waits for what follows it, so it reads with the human
    // input it accompanied instead of trailing the previous answer.
    if (isContext(block)) { held.set(block.scope, [...(held.get(block.scope) ?? []), block]); continue; }
    const waiting = held.get(block.scope) ?? [];
    held.delete(block.scope);
    let exchange = current.get(block.scope);
    if (readingRole(block.msg) === "human") {
      // Consecutive human messages in one recorded call stay together.
      const adjacent = exchange && exchange.prompts.at(-1)?.spanId === block.spanId && exchange.blocks.every(isContext);
      if (!exchange || !adjacent) exchange = open(block);
      exchange.blocks.push(...waiting);
      exchange.prompts.push(block);
    } else {
      exchange ??= open(block);
      exchange.blocks.push(...waiting, block);
    }
  }
  for (const [scope, waiting] of held) current.get(scope)?.blocks.push(...waiting);
  // Context-only setup is accessible in the trace; it isn't an empty exchange.
  return exchanges.filter(e => e.prompts.length || e.blocks.some(b => !isContext(b)));
}

export const responseFor = (exchange: Exchange) => exchange.blocks.filter(b => b.msg.role === "assistant" && messageText(b.msg).trim()).at(-1);
export const defaultExchange = (exchanges: Exchange[]) => exchanges.filter(e => !e.child).at(-1) ?? exchanges.at(-1);
// The readable text a person typed, or "" when the prompt carries none.
export const promptText = (e: Exchange) => e.prompts.map(p => messageText(p.msg)).filter(t => t.trim() && !HARNESS_WRAPPER.test(t)).at(-1)?.trim() ?? "";
// What the prompt list shows: the text, or what the prompt contains instead.
export const promptLabel = (e: Exchange) => {
  const text = promptText(e);
  if (text) return text;
  const types = new Set(e.prompts.flatMap(p => p.msg.content.map(part => part.type)));
  return types.has("tool_result") ? "Tool result" : types.has("image") ? "Image" : "Human input without text";
};
// A one-line preview for lists and outlines, without Markdown syntax.
export const previewText = (text: string, max = 86) => shortText(text
  .replace(/```\w*/g, " ")
  .replace(/`([^`]*)`/g, "$1")
  .replace(/!?\[([^\]]*)\]\([^)]*\)/g, "$1")
  .replace(/(\*\*|__)(.+?)\1/g, "$2")
  .replace(/^\s{0,3}(?:#{1,6}\s+|[-*+]\s+|\d+[.)]\s+|>\s?)/gm, "")
  .replace(/\|?\s*:?-{3,}:?\s*/g, " ").replace(/\|/g, " "), max);
export const shortText = (text: string, max = 86) => { const clean = text.replace(/\s+/g, " ").trim(); return clean.length > max ? clean.slice(0, max - 1).trimEnd() + "…" : clean; };
// A recorded time for display: time of day, with the date only when it falls
// on a different day from the session start. The title carries the full
// date, time and zone.
export function eventTime(at?: string, sessionStart?: string, seconds = false) {
  const date = at ? new Date(at) : null;
  if (!date || Number.isNaN(date.getTime())) return null;
  const clock = date.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit", ...(seconds ? { second: "2-digit" } : {}) });
  const start = sessionStart ? new Date(sessionStart) : null;
  const sameDay = !!start && !Number.isNaN(start.getTime()) && start.toDateString() === date.toDateString();
  return {
    iso: date.toISOString(),
    label: sameDay ? clock : `${date.toLocaleDateString(undefined, { month: "short", day: "numeric" })}, ${clock}`,
    title: date.toLocaleString(undefined, { dateStyle: "full", timeStyle: "long" }),
  };
}
// Compact elapsed time between two recorded moments, or "" when unknown.
export function elapsed(from?: string, to?: string): string {
  const ms = from && to ? new Date(to).getTime() - new Date(from).getTime() : NaN;
  if (!(ms >= 1000)) return "";
  const s = Math.round(ms / 1000), m = Math.floor(s / 60), h = Math.floor(m / 60);
  return h ? `${h}h ${m % 60}m` : m >= 10 ? `${m}m` : m ? `${m}m ${s % 60}s` : `${s}s`;
}
export const meaningfulTitle = (name?: string) => !!name?.trim() && !/^(?:untitled(?: session)?|session|[a-f\d-]{24,}|rollout-.*|.*\.(?:jsonl?|spool))$/i.test(name.trim()) && !/^[/<]/.test(name.trim());
// Importers clip the first prompt into `name` when a harness records no
// summary. That keeps lists readable, but it is not a title: the reader
// already opens on that prompt. Treat such names as untitled.
const normalizeText = (text: string) => text.replace(/\s+/g, " ").trim().toLowerCase();
export function nameIsFirstPrompt(name: string, exchanges: Exchange[]): boolean {
  const stem = normalizeText(name.replace(/(?:…|\.\.\.)$/, ""));
  if (!stem) return true;
  const first = exchanges.filter(e => !e.child).flatMap(e => e.prompts.map(p => normalizeText(messageText(p.msg)))).find(t => t && !t.startsWith("<"));
  return !!first && (first === stem || (stem.length >= 20 && first.startsWith(stem)));
}

// The authored or harness-provided title, or "" when the session is untitled.
export function sessionTitle(run: Run, exchanges: Exchange[]): string {
  const name = run.name?.trim() ?? "";
  return name && meaningfulTitle(name) && !nameIsFirstPrompt(name, exchanges) ? name : "";
}

const HARNESS_NAMES: Record<string, string> = { "claude-code": "Claude Code", codex: "Codex", pi: "pi", opencode: "OpenCode", hermes: "Hermes" };
export function harnessName(run: Run): string {
  const harness = (run.source as { harness?: string } | undefined)?.harness;
  if (harness) return HARNESS_NAMES[harness] ?? harness;
  const kind = run.source?.kind;
  return kind ? kind[0].toUpperCase() + kind.slice(1) : "Session";
}
const MONTHS = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
export function sessionDate(run: Run): string {
  const date = new Date(run.created_at);
  return Number.isNaN(date.getTime()) ? run.created_at.slice(0, 10) : `${MONTHS[date.getMonth()]} ${date.getDate()}, ${date.getFullYear()}`;
}
// What an untitled session is called until someone names it.
export const sessionLabel = (run: Run) => `Untitled · ${harnessName(run)} · ${sessionDate(run)}`;

// Only a fingerprint and reading coordinates enter browser storage, never
// captured text. Local previews use their full content-addressed source ID.
export function readingKey(run: Run, exchanges: Exchange[]): string {
  let hash = 2166136261;
  for (const text of [run.created_at, run.name ?? "", ...exchanges.flatMap(e => [e.id, ...e.prompts.map(p => messageText(p.msg)), ...e.blocks.map(b => messageText(b.msg))])]) {
    for (let i = 0; i < text.length; i++) hash = Math.imul(hash ^ text.charCodeAt(i), 16777619);
  }
  return (hash >>> 0).toString(16);
}
