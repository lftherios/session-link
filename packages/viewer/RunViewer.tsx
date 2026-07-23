"use client";

import { Component, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import {
  Bot,
  Braces,
  Check,
  ChevronDown,
  ChevronRight,
  Copy,
  Search,
  User,
  Wrench,
} from "lucide-react";
import type { ContentPart, Message, Run, Span } from "@session-link/format";

/* ---------------------------------------------------------------- tokens */

/* Every color is a CSS custom property so the viewer follows the reader's
   theme: light by default, dark via prefers-color-scheme, and an embedding
   page's explicit data-theme wins in either direction. */
const T = {
  paper: "var(--rv-paper)",
  panel: "var(--rv-panel)",
  soft: "var(--rv-soft)",
  ink: "var(--rv-ink)",
  faint: "var(--rv-faint)",
  line: "var(--rv-line)",
  signal: "var(--rv-signal)",
  error: "var(--rv-error)",
  serif:
    '"Iowan Old Style","Palatino Linotype",Palatino,"Book Antiqua",Georgia,serif',
  sans: 'system-ui,-apple-system,"Segoe UI",Roboto,sans-serif',
  mono: 'ui-monospace,"SF Mono","Cascadia Code",Menlo,Consolas,monospace',
};

// Record<string, ...> on purpose: the wire format is open, so spans with
// types this build doesn't know can arrive — they take the `custom` hue.
const HUES: Record<string, string> = {
  llm_call: "var(--rv-hue-llm)",
  tool_call: "var(--rv-hue-tool)",
  retrieval: "var(--rv-hue-ret)",
  agent: "var(--rv-hue-agent)",
  custom: "var(--rv-faint)",
};

const spanHue = (s: Span) =>
  s.status === "error" ? T.error : (HUES[s.type] ?? HUES.custom);

const LIGHT_VARS = `--rv-paper:#f5f6f3;--rv-panel:#fdfdfb;--rv-soft:#eef1ec;--rv-ink:#17201c;--rv-faint:#5b6660;--rv-line:#d8ddd7;--rv-signal:#0e6f5c;--rv-error:#b3402e;--rv-hue-llm:#0e6f5c;--rv-hue-tool:#a16418;--rv-hue-ret:#28629c;--rv-hue-agent:#5b4b8a;--rv-selbg:rgba(14,111,92,.08);--rv-selection:rgba(14,111,92,.18);--rv-hover:rgba(23,32,28,.045);--rv-errbg:rgba(179,64,46,.06)`;
const DARK_VARS = `--rv-paper:#101512;--rv-panel:#171d19;--rv-soft:#1e2620;--rv-ink:#e3e9e4;--rv-faint:#8f9a93;--rv-line:#2b342d;--rv-signal:#3fae94;--rv-error:#e08070;--rv-hue-llm:#3fae94;--rv-hue-tool:#c9924a;--rv-hue-ret:#6aa3d8;--rv-hue-agent:#a08fd0;--rv-selbg:rgba(63,174,148,.12);--rv-selection:rgba(63,174,148,.25);--rv-hover:rgba(227,233,228,.05);--rv-errbg:rgba(224,128,112,.1)`;

const RV_CSS = `
.rv{${LIGHT_VARS}}
@media(prefers-color-scheme:dark){.rv{${DARK_VARS}}}
:root[data-theme="dark"] .rv{${DARK_VARS}}
:root[data-theme="light"] .rv{${LIGHT_VARS}}
.rv *{box-sizing:border-box}
.rv button{font:inherit}
.rv pre{margin:0}
.rv ::selection{background:var(--rv-selection)}
.rv .rv-row:hover{background:var(--rv-hover)}
.rv .rv-flowblock{border-radius:6px;padding:0 8px;margin:0 -8px;cursor:pointer}
.rv .rv-flowblock:hover{background:var(--rv-hover)}
.rv :where(button,[role=button],[role=treeitem]):focus-visible{outline:2px solid var(--rv-signal);outline-offset:2px;border-radius:4px}
.rv .rv-cols{display:flex;border:1px solid var(--rv-line);border-radius:10px;background:var(--rv-panel);overflow:hidden}
.rv .rv-tree{width:340px;flex-shrink:0;border-right:1px solid var(--rv-line);display:flex;flex-direction:column}
@keyframes rv-pulse{0%,55%{background:var(--rv-selection)}100%{background:transparent}}
.rv .rv-linked{animation:rv-pulse 1.5s ease 2}
.rv .rv-visually-hidden{position:absolute;width:1px;height:1px;overflow:hidden;clip:rect(0 0 0 0);white-space:nowrap}
@media(max-width:880px){
  .rv .rv-cols{flex-direction:column}
  .rv .rv-tree{width:100%;border-right:none;border-bottom:1px solid var(--rv-line)}
}
@media(prefers-reduced-motion:no-preference){.rv .rv-row{transition:background .12s ease}}
@media(prefers-reduced-motion:reduce){.rv .rv-linked{animation:none;background:var(--rv-selection)}}
`;

/* --------------------------------------------------------------- helpers */

const ts = (s?: string) => (s ? new Date(s).getTime() : NaN);

function fmtDur(msVal: number): string {
  if (!Number.isFinite(msVal)) return "";
  if (msVal < 1000) return `${Math.round(msVal)}ms`;
  const s = msVal / 1000;
  if (s < 60) return `${s.toFixed(2)}s`;
  const m = Math.floor(s / 60);
  if (m < 60) return `${m}m ${String(Math.round(s % 60)).padStart(2, "0")}s`;
  const h = Math.floor(m / 60);
  return `${h}h ${String(m % 60).padStart(2, "0")}m`;
}

const fmtInt = (n: number) => n.toLocaleString("en-US");

const fmtCost = (n: number) => (n < 0.01 ? `$${n.toFixed(4)}` : `$${n.toFixed(2)}`);

const spanDur = (s: Span) => ts(s.ended_at) - ts(s.started_at);

// Coerced defensively: the wire format is open, so `name` and `type` can
// arrive as anything — a non-string label must not throw in every tree row.
const spanLabel = (s: Span) =>
  (typeof s.name === "string" && s.name) ||
  (typeof s.type === "string" && s.type) ||
  "span";

function stringify(v: unknown): string {
  try {
    return JSON.stringify(v, null, 2) ?? String(v);
  } catch {
    return String(v);
  }
}

interface RunIndex {
  roots: Span[];
  children: Map<string, Span[]>;
  byId: Map<string, Span>;
  parents: Map<string, string>;
  /** span ids with status === "error", in document order */
  errors: string[];
  /** llm_call span ids in document order — the substantive turns */
  llmCalls: string[];
  t0: number;
  t1: number;
  models: string[];
  tokensIn?: number;
  tokensOut?: number;
  tokensCached?: number;
  cost?: number;
  durMs?: number;
}

/** First few words of a span's own content — what a tree row can show so
 *  600 rows named "turn" stop being identical. Cached per span object:
 *  TreeRow renders on every scroll frame, and stringifying a 1MB tool arg
 *  40 times per frame is a jank cliff. */
const previewCache = new WeakMap<Span, string>();

function spanPreview(s: Span): string {
  const hit = previewCache.get(s);
  if (hit !== undefined) return hit;
  const firstText = (msgs?: Message[]): string => {
    for (const m of msgs ?? []) {
      for (const p of m.content ?? []) {
        if (p && typeof p === "object" && (p as { type?: unknown }).type === "text") {
          const t = (p as { text?: unknown }).text;
          if (typeof t === "string" && t.trim()) return t;
        }
      }
    }
    return "";
  };
  let text = "";
  if (s.type === "llm_call") {
    text = firstText((s as { output?: { messages?: Message[] } }).output?.messages);
    if (!text) text = firstText((s as { input?: { messages?: Message[] } }).input?.messages);
  } else if (s.type === "tool_call") {
    const args = (s as { input?: { arguments?: unknown } }).input?.arguments;
    if (args != null) {
      // The preview keeps 64 chars — never stringify more than a slice.
      const raw = typeof args === "string" ? args : stringify(args);
      text = raw.slice(0, 512).replace(/[{}"\n]/g, " ");
    }
  }
  text = text.slice(0, 512).replace(/\s+/g, " ").trim();
  const out = text === "" ? "" : text.length > 64 ? text.slice(0, 63) + "…" : text;
  previewCache.set(s, out);
  return out;
}

function indexRun(run: Run): RunIndex {
  const byId = new Map<string, Span>();
  for (const s of run.spans) byId.set(s.id, s);

  const children = new Map<string, Span[]>();
  const parents = new Map<string, string>();
  const roots: Span[] = [];
  const errors: string[] = [];
  const llmCalls: string[] = [];
  for (const s of run.spans) {
    const pid = s.parent_id ?? null;
    if (pid && byId.has(pid)) {
      const list = children.get(pid) ?? [];
      list.push(s);
      children.set(pid, list);
      parents.set(s.id, pid);
    } else {
      roots.push(s);
    }
    if (s.status === "error") errors.push(s.id);
    if (s.type === "llm_call") llmCalls.push(s.id);
  }

  let t0 = Infinity;
  let t1 = -Infinity;
  for (const s of run.spans) {
    const a = ts(s.started_at);
    const b = ts(s.ended_at);
    if (Number.isFinite(a)) t0 = Math.min(t0, a);
    if (Number.isFinite(b)) t1 = Math.max(t1, b);
  }
  const hasTimeline = Number.isFinite(t0) && Number.isFinite(t1) && t1 > t0;

  const models: string[] = [];
  let tin = 0;
  let tout = 0;
  let tcached = 0;
  let cost = 0;
  let sawUsage = false;
  let sawCached = false;
  let sawCost = false;
  for (const s of run.spans) {
    if (s.type !== "llm_call") continue;
    // Open-world guard: a span may claim llm_call without carrying a model.
    if (s.model) {
      const label = `${s.model.provider ? s.model.provider + "/" : ""}${s.model.id}`;
      if (!models.includes(label)) models.push(label);
    }
    if (s.usage) {
      sawUsage = true;
      tin += s.usage.input_tokens ?? 0;
      tout += s.usage.output_tokens ?? 0;
      if (s.usage.cache_read_tokens != null) {
        sawCached = true;
        tcached += s.usage.cache_read_tokens;
      }
      if (s.usage.cost_usd != null) {
        sawCost = true;
        cost += s.usage.cost_usd;
      }
    }
  }

  return {
    roots,
    children,
    byId,
    parents,
    errors,
    llmCalls,
    t0,
    t1,
    models,
    tokensIn: run.metrics?.input_tokens ?? (sawUsage ? tin : undefined),
    tokensOut: run.metrics?.output_tokens ?? (sawUsage ? tout : undefined),
    tokensCached: sawCached ? tcached : undefined,
    cost: run.metrics?.cost_usd ?? (sawCost ? cost : undefined),
    durMs: run.metrics?.latency_ms ?? (hasTimeline ? t1 - t0 : undefined),
  };
}

/* ------------------------------------------------------------ boundaries */

/**
 * The format is open-world, so partial or odd documents are normal — data
 * this build can't render must degrade to a card, never a white screen.
 */
class Boundary extends Component<
  { fallback: (message: string) => React.ReactNode; children: React.ReactNode },
  { message: string | null }
> {
  state: { message: string | null } = { message: null };
  static getDerivedStateFromError(e: unknown) {
    return { message: e instanceof Error ? e.message : String(e) };
  }
  render() {
    if (this.state.message != null) return this.props.fallback(this.state.message);
    return this.props.children;
  }
}

/** Whole-viewer fallback — the calm version of a crash. */
function ViewerFallback({ message, src }: { message: string; src?: string }) {
  return (
    <div
      className="rv"
      style={{
        fontFamily: T.mono,
        fontSize: 12,
        color: T.faint,
        border: `1px solid ${T.line}`,
        borderRadius: 10,
        background: T.panel,
        padding: "48px 24px",
        textAlign: "center",
      }}
    >
      <div style={{ color: T.error }}>
        this session couldn&apos;t be rendered — {message}
      </div>
      <div style={{ marginTop: 8 }}>
        {src ? (
          <>
            the raw JSON is still valid — only this viewer build choked on it
            {" · "}
            <a href={src} style={{ color: T.ink }}>
              raw JSON
            </a>
          </>
        ) : (
          /* No URL to point at (e.g. the run was inlined into the page) —
             don't promise an escape hatch this card can't render. */
          <>the document itself is intact — only this viewer build choked on it</>
        )}
      </div>
    </div>
  );
}

/** Per-span fallback — replaces one span's detail, never the tree. */
function SpanFallback({ message }: { message: string }) {
  return (
    <div
      style={{
        border: `1px dashed ${T.line}`,
        borderRadius: 6,
        padding: "10px 12px",
        fontFamily: T.mono,
        fontSize: 12,
        color: T.faint,
      }}
    >
      <span style={{ color: T.error }}>this span couldn&apos;t be rendered</span>
      {" — "}
      {message} · the raw view still works
    </div>
  );
}

/* ---------------------------------------------------------------- atoms */

function Eyebrow({ children }: { children: React.ReactNode }) {
  return (
    <div
      style={{
        fontFamily: T.mono,
        fontSize: 11,
        letterSpacing: "0.14em",
        textTransform: "uppercase",
        color: T.faint,
      }}
    >
      {children}
    </div>
  );
}

function Chip({
  children,
  mono = true,
  title,
}: {
  children: React.ReactNode;
  mono?: boolean;
  title?: string;
}) {
  return (
    <span
      title={title}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 6,
        border: `1px solid ${T.line}`,
        borderRadius: 999,
        padding: "3px 10px",
        fontFamily: mono ? T.mono : T.sans,
        fontSize: 12,
        color: T.ink,
        background: T.panel,
      }}
    >
      {children}
    </span>
  );
}

function Metric({ label, value }: { label: string; value: string }) {
  return (
    <div style={{ minWidth: 84 }}>
      <div style={{ fontFamily: T.mono, fontSize: 15 }}>{value}</div>
      <div
        style={{
          fontFamily: T.mono,
          fontSize: 10,
          letterSpacing: "0.12em",
          textTransform: "uppercase",
          color: T.faint,
          marginTop: 2,
        }}
      >
        {label}
      </div>
    </div>
  );
}

function SectionLabel({ children }: { children: React.ReactNode }) {
  return (
    <div
      style={{
        fontFamily: T.mono,
        fontSize: 10,
        letterSpacing: "0.14em",
        textTransform: "uppercase",
        color: T.faint,
        margin: "18px 0 8px",
      }}
    >
      {children}
    </div>
  );
}

/** Placeholder for a content-addressed payload whose bytes live in the
 *  attachments manifest. Mirrors the image-attachment placeholder. */
function BlobRefNote({ label, hash }: { label: string; hash: string }) {
  return (
    <div
      style={{
        fontFamily: T.mono,
        fontSize: 11,
        color: T.faint,
        border: `1px dashed ${T.line}`,
        borderRadius: 6,
        padding: "8px 12px",
      }}
    >
      {label} · {hash.slice(0, 26)}… (content-addressed, bytes not inlined)
    </div>
  );
}

/** Past either bound, text mounts clamped behind an expander — one huge
 *  tool result must not swamp the page (or the DOM) it lands on. */
const CLAMP_LINES = 20;
const CLAMP_CHARS = 4 * 1024;

function ClampedText({
  text,
  render,
}: {
  text: string;
  render: (visible: string) => React.ReactNode;
}) {
  const [showAll, setShowAll] = useState(false);
  const lines = text.split("\n").length;
  if (lines <= CLAMP_LINES && text.length <= CLAMP_CHARS) {
    return <>{render(text)}</>;
  }
  const clipped =
    text.slice(0, CLAMP_CHARS).split("\n").slice(0, CLAMP_LINES).join("\n") +
    "\n…";
  const kb = `${(text.length / 1024).toFixed(1)} KB`;
  return (
    <div style={{ display: "grid", gap: 6 }}>
      {render(showAll ? text : clipped)}
      <button
        onClick={() => setShowAll((v) => !v)}
        style={{
          justifySelf: "start",
          border: `1px solid ${T.line}`,
          background: T.panel,
          borderRadius: 6,
          padding: "4px 9px",
          fontFamily: T.mono,
          fontSize: 11,
          cursor: "pointer",
          color: T.faint,
        }}
      >
        {showAll
          ? "collapse"
          : lines > 1
            ? `show all ${fmtInt(lines)} lines (${kb})`
            : `show all ${kb}`}
      </button>
    </div>
  );
}

function JsonBlock({ value, tall = false }: { value: unknown; tall?: boolean }) {
  return (
    <ClampedText
      text={stringify(value)}
      render={(visible) => (
        <pre
          style={{
            fontFamily: T.mono,
            fontSize: 12,
            lineHeight: 1.55,
            background: T.soft,
            borderRadius: 6,
            padding: "10px 12px",
            overflow: "auto",
            maxHeight: tall ? 560 : 280,
            whiteSpace: "pre",
          }}
        >
          {visible}
        </pre>
      )}
    />
  );
}

/* --------------------------------------------------------- markdown-lite */

/**
 * Markdown-lite: fenced code blocks, inline code, and links — the three
 * constructs that make a coding-agent transcript readable. Deliberately not
 * CommonMark. Output is React elements only (never raw HTML), so session
 * content can't inject markup no matter what it contains; links render for
 * http(s) URLs only.
 */
function CopyButton({ text, label = "copy" }: { text: string; label?: string }) {
  const [done, setDone] = useState(false);
  return (
    <button
      onClick={() => {
        // "copied" only when the write actually landed.
        navigator.clipboard
          ?.writeText(text)
          .then(() => {
            setDone(true);
            window.setTimeout(() => setDone(false), 1400);
          })
          .catch(() => {});
      }}
      aria-label={`Copy ${label}`}
      style={{
        display: "inline-flex",
        alignItems: "center",
        gap: 4,
        border: `1px solid ${T.line}`,
        background: T.panel,
        borderRadius: 5,
        padding: "2px 7px",
        fontFamily: T.mono,
        fontSize: 10,
        cursor: "pointer",
        color: done ? T.signal : T.faint,
      }}
    >
      {done ? <Check size={10} /> : <Copy size={10} />}
      {done ? "copied" : label}
    </button>
  );
}

function InlineMd({ text }: { text: string }) {
  // Split on inline code first (code wins over links inside it), then
  // linkify http(s) URLs and [label](url) in the remaining text runs.
  const out: React.ReactNode[] = [];
  const codeSplit = text.split(/(`[^`\n]+`)/);
  codeSplit.forEach((seg, i) => {
    if (i % 2 === 1) {
      out.push(
        <code
          key={i}
          style={{
            fontFamily: T.mono,
            fontSize: "0.92em",
            background: T.soft,
            borderRadius: 4,
            padding: "1px 5px",
          }}
        >
          {seg.slice(1, -1)}
        </code>,
      );
      return;
    }
    const linkRe = /\[([^\]\n]+)\]\((https?:\/\/[^)\s]+)\)|(https?:\/\/[^\s<>"')\]]+)/g;
    let last = 0;
    let m: RegExpExecArray | null;
    while ((m = linkRe.exec(seg)) !== null) {
      if (m.index > last) out.push(seg.slice(last, m.index));
      const label = m[1] ?? m[3] ?? "";
      const href = m[2] ?? m[3] ?? "";
      out.push(
        <a
          key={`${i}-${m.index}`}
          href={href}
          target="_blank"
          rel="noopener noreferrer"
          style={{ color: T.signal, textDecorationThickness: 1 }}
        >
          {label}
        </a>,
      );
      last = m.index + m[0].length;
    }
    if (last < seg.length) out.push(seg.slice(last));
  });
  return <>{out}</>;
}

function MdText({ text }: { text: string }) {
  // Single-pass line scanner — a backtracking fence regex goes quadratic
  // on unterminated-opener text (2,000 stray ``` lines froze the tab for
  // seconds exactly when the reader clicked "show all").
  const segs: string[] = [];
  {
    const lines = text.split("\n");
    let prose: string[] = [];
    let i = 0;
    while (i < lines.length) {
      const open = lines[i].match(/^```([^\n]*)$/);
      if (open) {
        let j = i + 1;
        while (j < lines.length && !/^```[ \t]*$/.test(lines[j])) j++;
        if (j < lines.length) {
          segs.push(prose.join("\n"), open[1], lines.slice(i + 1, j).join("\n"));
          prose = [];
          i = j + 1;
          continue;
        }
        // Unterminated fence: it's prose, like the old regex treated it.
      }
      prose.push(lines[i]);
      i++;
    }
    segs.push(prose.join("\n"));
  }
  const nodes: React.ReactNode[] = [];
  for (let i = 0; i < segs.length; i += 3) {
    const prose = segs[i];
    if (prose && prose.trim()) {
      nodes.push(
        <div key={`p${i}`} style={{ whiteSpace: "pre-wrap" }}>
          <InlineMd text={prose.replace(/^\n+|\n+$/g, "")} />
        </div>,
      );
    }
    if (i + 2 < segs.length) {
      const lang = (segs[i + 1] ?? "").trim();
      const code = (segs[i + 2] ?? "").replace(/\n$/, "");
      nodes.push(
        <div key={`c${i}`} style={{ position: "relative" }}>
          <pre
            style={{
              fontFamily: T.mono,
              fontSize: 12,
              lineHeight: 1.55,
              background: T.soft,
              borderRadius: 6,
              padding: "10px 12px",
              overflow: "auto",
              whiteSpace: "pre",
            }}
          >
            {code}
          </pre>
          <div style={{ position: "absolute", top: 6, right: 6, display: "flex", gap: 6, alignItems: "center" }}>
            {lang && (
              <span style={{ fontFamily: T.mono, fontSize: 10, color: T.faint }}>{lang}</span>
            )}
            <CopyButton text={code} label="copy" />
          </div>
        </div>,
      );
    }
  }
  return <div style={{ display: "grid", gap: 8, fontSize: 13.5, lineHeight: 1.6 }}>{nodes}</div>;
}

/* ----------------------------------------------------- message rendering */

function PartView({ part }: { part: ContentPart }) {
  switch (part.type) {
    case "text":
      return (
        <ClampedText
          text={part.text}
          render={(visible) => <MdText text={visible} />}
        />
      );
    case "thinking":
      return (
        <details>
          <summary
            style={{
              cursor: "pointer",
              fontFamily: T.mono,
              fontSize: 11,
              letterSpacing: "0.1em",
              textTransform: "uppercase",
              color: T.faint,
            }}
          >
            thinking
          </summary>
          <ClampedText
            text={part.text}
            render={(visible) => (
              <div
                style={{
                  fontFamily: T.serif,
                  fontStyle: "italic",
                  fontSize: 13.5,
                  lineHeight: 1.6,
                  color: T.faint,
                  whiteSpace: "pre-wrap",
                  borderLeft: `2px solid ${T.line}`,
                  padding: "4px 0 4px 12px",
                  margin: "8px 0 0",
                }}
              >
                {visible}
              </div>
            )}
          />
        </details>
      );
    case "tool_call":
      return (
        <div style={{ border: `1px solid ${T.line}`, borderRadius: 6 }}>
          <div
            style={{
              display: "flex",
              gap: 8,
              alignItems: "baseline",
              padding: "6px 12px",
              borderBottom: `1px solid ${T.line}`,
              fontFamily: T.mono,
              fontSize: 12,
            }}
          >
            <span style={{ color: HUES.tool_call }}>→ {part.name}</span>
            <span style={{ color: T.faint }}>{part.id}</span>
          </div>
          <div style={{ padding: 10 }}>
            <JsonBlock value={part.arguments} />
          </div>
        </div>
      );
    case "tool_result":
      return (
        <div style={{ border: `1px solid ${T.line}`, borderRadius: 6 }}>
          <div
            style={{
              display: "flex",
              gap: 8,
              alignItems: "baseline",
              padding: "6px 12px",
              borderBottom: `1px solid ${T.line}`,
              fontFamily: T.mono,
              fontSize: 12,
            }}
          >
            <span style={{ color: part.is_error ? T.error : HUES.tool_call }}>
              ← result
            </span>
            <span style={{ color: T.faint }}>{part.tool_call_id}</span>
            {part.is_error && <span style={{ color: T.error }}>error</span>}
          </div>
          <div style={{ padding: 10, display: "grid", gap: 8 }}>
            {part.content.map((p, i) => (
              <PartView key={i} part={p} />
            ))}
          </div>
        </div>
      );
    case "image":
      return part.url ? (
        // eslint-disable-next-line @next/next/no-img-element
        <img
          src={part.url}
          alt="attachment"
          style={{
            maxWidth: 360,
            borderRadius: 6,
            border: `1px solid ${T.line}`,
            display: "block",
          }}
        />
      ) : (
        <div
          style={{
            fontFamily: T.mono,
            fontSize: 11,
            color: T.faint,
            border: `1px dashed ${T.line}`,
            borderRadius: 6,
            padding: "8px 12px",
          }}
        >
          image · {(part.attachment ?? "no source").slice(0, 26)}…
        </div>
      );
    case "data":
      return <JsonBlock value={part.data} />;
    case "blob":
      return (
        <BlobRefNote
          label={part.name ?? part.mime ?? "blob"}
          hash={part.attachment}
        />
      );
    default:
      /* progressive enhancement in miniature: never drop unknown parts */
      return <JsonBlock value={part} />;
  }
}

function RoleGutter({ role }: { role: Message["role"] }) {
  const icon =
    role === "assistant" ? (
      <Bot size={13} />
    ) : role === "user" ? (
      <User size={13} />
    ) : role === "tool" ? (
      <Wrench size={13} />
    ) : (
      <Braces size={13} />
    );
  return (
    <div
      style={{
        width: 88,
        flexShrink: 0,
        display: "flex",
        alignItems: "center",
        gap: 6,
        fontFamily: T.mono,
        fontSize: 10,
        letterSpacing: "0.12em",
        textTransform: "uppercase",
        color: T.faint,
        paddingTop: 2,
      }}
    >
      {icon}
      {role}
    </div>
  );
}

const messageText = (msg: Message): string =>
  (msg.content ?? [])
    .filter((p): p is Extract<ContentPart, { type: "text" }> => (p as { type?: unknown })?.type === "text")
    .map((p) => p.text)
    .join("\n\n");

function MessageView({ msg }: { msg: Message }) {
  const txt = messageText(msg);
  return (
    <div
      style={{
        display: "flex",
        gap: 12,
        padding: "12px 0",
        borderTop: `1px solid ${T.line}`,
        alignItems: "flex-start",
      }}
    >
      <RoleGutter role={msg.role} />
      <div style={{ display: "grid", gap: 10, minWidth: 0, flex: 1 }}>
        {msg.content.map((p, i) => (
          <PartView key={i} part={p} />
        ))}
      </div>
      {txt && (
        <div style={{ flexShrink: 0, paddingTop: 2 }}>
          <CopyButton text={txt} label="copy" />
        </div>
      )}
    </div>
  );
}

/* ------------------------------------------------------------ span detail */

function LlmDetail({ span }: { span: Extract<Span, { type: "llm_call" }> }) {
  return (
    <>
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: 6,
          alignItems: "center",
        }}
      >
        <Chip>
          {span.model.provider ? `${span.model.provider}/` : ""}
          {span.model.id}
        </Chip>
        {span.params &&
          Object.entries(span.params).map(([k, v]) => (
            <span
              key={k}
              style={{
                fontFamily: T.mono,
                fontSize: 11.5,
                color: T.faint,
                background: T.soft,
                borderRadius: 5,
                padding: "3px 8px",
              }}
            >
              {k}{" "}
              <span style={{ color: T.ink }}>
                {typeof v === "object" ? stringify(v) : String(v)}
              </span>
            </span>
          ))}
      </div>

      {span.input.system ? (
        <>
          <SectionLabel>system</SectionLabel>
          <div
            style={{
              fontSize: 13,
              lineHeight: 1.6,
              whiteSpace: "pre-wrap",
              background: T.soft,
              border: `1px solid ${T.line}`,
              borderRadius: 6,
              padding: "10px 12px",
            }}
          >
            {span.input.system}
          </div>
        </>
      ) : (
        span.input.system_ref && (
          <>
            <SectionLabel>system</SectionLabel>
            <BlobRefNote label="system" hash={span.input.system_ref} />
          </>
        )
      )}

      {span.input.tools && span.input.tools.length > 0 ? (
        <>
          <SectionLabel>tools</SectionLabel>
          <div style={{ display: "flex", flexWrap: "wrap", gap: 6 }}>
            {span.input.tools.map((t) => (
              <Chip key={t.name} title={t.description}>
                <Wrench size={11} /> {t.name}
              </Chip>
            ))}
          </div>
        </>
      ) : (
        span.input.tools_ref && (
          <>
            <SectionLabel>tools</SectionLabel>
            <BlobRefNote label="tools" hash={span.input.tools_ref} />
          </>
        )
      )}

      <SectionLabel>input</SectionLabel>
      <div>
        {(span.input.messages ?? []).map((m, i) => (
          <MessageView key={i} msg={m} />
        ))}
        {!span.input.messages && span.input.messages_ref && (
          <BlobRefNote label="messages" hash={span.input.messages_ref} />
        )}
      </div>

      <SectionLabel>output</SectionLabel>
      <div>
        {span.output.messages.map((m, i) => (
          <MessageView key={i} msg={m} />
        ))}
      </div>

      {span.usage && (
        <div
          style={{
            marginTop: 16,
            paddingTop: 12,
            borderTop: `1px solid ${T.line}`,
            fontFamily: T.mono,
            fontSize: 12,
            color: T.faint,
          }}
        >
          {span.usage.input_tokens != null && `${fmtInt(span.usage.input_tokens)} in`}
          {span.usage.output_tokens != null &&
            ` · ${fmtInt(span.usage.output_tokens)} out`}
          {span.usage.cache_read_tokens != null &&
            ` · ${fmtInt(span.usage.cache_read_tokens)} cached`}
          {span.usage.cost_usd != null && ` · ${fmtCost(span.usage.cost_usd)}`}
        </div>
      )}
    </>
  );
}

function SpanDetail({ span }: { span: Span }) {
  switch (span.type) {
    case "llm_call":
      return <LlmDetail span={span} />;
    case "tool_call":
      return (
        <>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <Chip>
              <Wrench size={11} /> {span.input.name}
            </Chip>
            {span.input.tool_call_id && (
              <span style={{ fontFamily: T.mono, fontSize: 12, color: T.faint }}>
                {span.input.tool_call_id}
              </span>
            )}
          </div>
          <SectionLabel>arguments</SectionLabel>
          <JsonBlock value={span.input.arguments} />
          <SectionLabel>result</SectionLabel>
          {span.output?.result === undefined && span.output?.result_ref ? (
            <BlobRefNote label="result" hash={span.output.result_ref} />
          ) : (
            <JsonBlock value={span.output?.result} />
          )}
        </>
      );
    case "retrieval":
      return (
        <>
          <div style={{ display: "flex", gap: 8, alignItems: "center" }}>
            <Search size={13} color={HUES.retrieval} />
            <span style={{ fontFamily: T.mono, fontSize: 13 }}>
              {span.input.query}
            </span>
          </div>
          <SectionLabel>documents</SectionLabel>
          <div style={{ display: "grid", gap: 8 }}>
            {(span.output?.documents ?? []).map((d, i) => (
              <div
                key={i}
                style={{
                  border: `1px solid ${T.line}`,
                  borderRadius: 6,
                  padding: "8px 12px",
                }}
              >
                <div
                  style={{
                    display: "flex",
                    justifyContent: "space-between",
                    fontFamily: T.mono,
                    fontSize: 11,
                    color: T.faint,
                    marginBottom: d.text ? 6 : 0,
                  }}
                >
                  <span>{d.ref ?? `doc ${i + 1}`}</span>
                  {d.score != null && <span>{d.score.toFixed(3)}</span>}
                </div>
                {d.text && (
                  <div
                    style={{
                      fontSize: 13,
                      lineHeight: 1.55,
                      whiteSpace: "pre-wrap",
                      maxHeight: 120,
                      overflow: "auto",
                    }}
                  >
                    {d.text}
                  </div>
                )}
              </div>
            ))}
          </div>
        </>
      );
    default:
      return (
        <>
          {span.input != null && (
            <>
              <SectionLabel>input</SectionLabel>
              <JsonBlock value={span.input} />
            </>
          )}
          {span.output != null && (
            <>
              <SectionLabel>output</SectionLabel>
              <JsonBlock value={span.output} />
            </>
          )}
        </>
      );
  }
}

/* -------------------------------------------------------------- the tree */

function TreeRow({
  span,
  depth,
  idx,
  posinset,
  setsize,
  selected,
  linked = false,
  hasKids,
  open,
  onToggle,
  onSelect,
}: {
  span: Span;
  depth: number;
  idx: RunIndex;
  posinset?: number;
  setsize?: number;
  selected: boolean;
  linked?: boolean;
  hasKids: boolean;
  open: boolean;
  onToggle: () => void;
  onSelect: () => void;
}) {
  const dur = spanDur(span);
  const hasTimeline = idx.t1 > idx.t0;
  const a = ts(span.started_at);
  const showBar = hasTimeline && Number.isFinite(a) && Number.isFinite(dur);
  const left = showBar ? ((a - idx.t0) / (idx.t1 - idx.t0)) * 100 : 0;
  const width = showBar ? Math.max(1.5, (dur / (idx.t1 - idx.t0)) * 100) : 0;
  const hue = spanHue(span);

  const preview = spanPreview(span);
  const isErr = span.status === "error";

  return (
    <div
      id={`rv-row-${span.id}`}
      role="treeitem"
      aria-selected={selected}
      aria-level={depth + 1}
      aria-posinset={posinset}
      aria-setsize={setsize}
      aria-expanded={hasKids ? open : undefined}
      // Not a tab stop: the scroll container is the single keyboard surface,
      // so focus can never strand on a row that scrolls out of the window.
      tabIndex={-1}
      title={
        span.started_at
          ? `${spanLabel(span)} · ${new Date(span.started_at).toLocaleTimeString()}${Number.isFinite(dur) ? ` · ${fmtDur(dur)}` : ""}`
          : spanLabel(span)
      }
      onClick={onSelect}
      onKeyDown={(e) => {
        if (e.key === "Enter" || e.key === " ") {
          e.preventDefault();
          onSelect();
        }
      }}
      className={linked ? "rv-row rv-linked" : "rv-row"}
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        height: ROW_H,
        padding: "0 12px 0 0",
        paddingLeft: 10 + depth * 14,
        cursor: "pointer",
        background: selected ? "var(--rv-selbg)" : "transparent",
        boxShadow: selected ? `inset 2px 0 0 ${T.signal}` : "none",
      }}
    >
      {hasKids ? (
        <button
          aria-label={open ? "Collapse" : "Expand"}
          // Not a tab stop — a focused chevron unmounting with its windowed
          // row would strand focus; the scroll container is the tab stop.
          tabIndex={-1}
          onClick={(e) => {
            e.stopPropagation();
            onToggle();
          }}
          style={{
            border: "none",
            background: "none",
            padding: 0,
            width: 16,
            height: 16,
            display: "flex",
            alignItems: "center",
            justifyContent: "center",
            cursor: "pointer",
            color: T.faint,
          }}
        >
          {open ? <ChevronDown size={13} /> : <ChevronRight size={13} />}
        </button>
      ) : (
        <span style={{ width: 16 }} />
      )}
      <span
        style={{
          width: 8,
          height: 8,
          borderRadius: 2,
          background: hue,
          flexShrink: 0,
        }}
      />
      <span
        style={{
          flex: 1,
          minWidth: 0,
          fontSize: 13,
          whiteSpace: "nowrap",
          overflow: "hidden",
          textOverflow: "ellipsis",
        }}
      >
        {isErr && (
          <span style={{ color: T.error, fontFamily: T.mono, fontSize: 11, marginRight: 5 }}>
            ✕
          </span>
        )}
        {spanLabel(span)}
        {preview && (
          <span style={{ color: T.faint }}> — {preview}</span>
        )}
      </span>
      {hasTimeline && (
        <span
          aria-hidden
          style={{
            width: 60,
            height: 4,
            borderRadius: 2,
            background: T.soft,
            position: "relative",
            flexShrink: 0,
            overflow: "hidden",
          }}
        >
          {showBar && (
            <span
              style={{
                position: "absolute",
                left: `${left}%`,
                width: `${width}%`,
                top: 0,
                bottom: 0,
                borderRadius: 2,
                background: hue,
              }}
            />
          )}
        </span>
      )}
      {hasTimeline && (
        <span
          style={{
            width: 54,
            textAlign: "right",
            fontFamily: T.mono,
            fontSize: 11,
            color: T.faint,
            flexShrink: 0,
          }}
        >
          {fmtDur(dur)}
        </span>
      )}
    </div>
  );
}

/* ------------------------------------------------------------- component */

/**
 * Renders a session/v0 document. Two ways in:
 *   run — the parsed document, inlined by the caller (local `slink open`,
 *         bundled examples, small sessions).
 *   src — a URL to fetch it from. The hosted site uses this for big
 *         sessions so the run's bytes travel once, gzipped, instead of
 *         being serialized into both the SSR markup and the hydration
 *         payload (an 11.8MB run made a 17.6MB page that way).
 * Passing both is meaningful: `run` wins for rendering (no fetch happens),
 * and `src` gives the crash fallback a raw-JSON link worth showing.
 */
export function RunViewer({ run, src }: { run?: Run; src?: string }) {
  const [fetched, setFetched] = useState<Run | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    if (run || !src) return;
    // A changed src must never show the previous run or a stale error.
    setFetched(null);
    setLoadError(null);
    let alive = true;
    fetch(src)
      .then(async (r) => {
        if (!r.ok) throw new Error(`the server answered ${r.status}`);
        return (await r.json()) as Run;
      })
      .then((data) => {
        if (alive) setFetched(data);
      })
      .catch((e: unknown) => {
        if (alive) setLoadError(e instanceof Error ? e.message : String(e));
      });
    return () => {
      alive = false;
    };
  }, [run, src]);

  const resolved = run ?? fetched;
  if (!resolved) {
    return (
      <div
        className="rv"
        style={{
          /* tokens must exist even before LoadedViewer mounts */
          fontFamily: T.mono,
          fontSize: 12,
          color: loadError ? T.error : T.faint,
          border: `1px solid ${T.line}`,
          borderRadius: 10,
          background: T.panel,
          padding: "48px 24px",
          textAlign: "center",
        }}
      >
        <style>{RV_CSS}</style>
        {loadError ? (
          <>
            couldn&apos;t load this session — {loadError}
            {src && (
              <>
                {" · "}
                <a href={src} style={{ color: T.ink }}>
                  raw JSON
                </a>
              </>
            )}
          </>
        ) : (
          "loading session…"
        )}
      </div>
    );
  }
  return (
    <>
      {/* Tokens + chrome live OUTSIDE the boundary so the crash card is
          styled even when LoadedViewer is what crashed. */}
      <style>{RV_CSS}</style>
      <Boundary
        fallback={(message) => <ViewerFallback message={message} src={src} />}
      >
        <LoadedViewer run={resolved} />
      </Boundary>
    </>
  );
}

/** Every visible tree row is exactly this tall — the invariant the
 *  windowing math depends on, enforced by an explicit height on the row. */
const ROW_H = 30;
const OVERSCAN = 12;
/** The list's cosmetic top padding — rows sit at PAD_TOP + i*ROW_H in
 *  content coordinates, so every scroll computation must include it. */
const PAD_TOP = 6;

/**
 * The left column: header, counts, and the windowed span list. Scroll state
 * lives HERE on purpose — a scroll frame re-renders these ~40 rows and
 * never the span-detail panel next door.
 */
/** Lower-cased searchable text per span: label, preview, type, tool name,
 *  error message, and the first ~1.5KB of message text — enough for a
 *  Cmd+F-style content search without holding the whole run twice. */
function buildSearchText(idx: RunIndex): Map<string, string> {
  const out = new Map<string, string>();
  for (const s of idx.byId.values()) {
    let text = `${spanLabel(s)} ${String(s.type)} `;
    if (s.status === "error") text += `error ${String(s.error?.message ?? "")} `;
    if (s.type === "tool_call") {
      const inp = (s as { input?: { name?: unknown; arguments?: unknown } }).input;
      text += `${String(inp?.name ?? "")} `;
      // Args and results are what tree rows preview — search must find
      // what the rows visibly show.
      if (inp?.arguments != null) {
        text += (typeof inp.arguments === "string" ? inp.arguments : stringify(inp.arguments)).slice(0, 600) + " ";
      }
      const res = (s as { output?: { result?: unknown } }).output?.result;
      if (res != null) {
        text += (typeof res === "string" ? res : stringify(res)).slice(0, 600) + " ";
      }
    }
    const grab = (msgs?: Message[]) => {
      for (const m of msgs ?? []) {
        if (text.length > 1500) return;
        text += messageText(m).slice(0, 400) + " ";
      }
    };
    if (s.type === "llm_call") {
      grab((s as { input?: { messages?: Message[] } }).input?.messages);
      grab((s as { output?: { messages?: Message[] } }).output?.messages);
    }
    out.set(s.id, text.toLowerCase());
  }
  return out;
}

function TreeColumn({
  idx,
  sel,
  closed,
  linkedId,
  onSelect,
  onSetClosed,
}: {
  idx: RunIndex;
  sel: string | null;
  closed: Set<string>;
  linkedId: string | null;
  onSelect: (id: string) => void;
  onSetClosed: React.Dispatch<React.SetStateAction<Set<string>>>;
}) {
  const [query, setQuery] = useState("");
  const [errOnly, setErrOnly] = useState(false);
  const searchRef = useRef<HTMLInputElement | null>(null);
  const searchText = useMemo(() => buildSearchText(idx), [idx]);
  const filtering = query.trim() !== "" || errOnly;

  /* The visible tree, flattened. Thousands of rows are normal for real
     agent traces, so only the window around the scroll position mounts —
     this array is cheap (plain objects), the DOM is what's rationed.
     While a search/filter is active the list becomes the flat matches in
     document order — a filter that respected collapse state would hide
     the very rows being searched for. */
  const rows = useMemo(() => {
    if (filtering) {
      const q = query.trim().toLowerCase();
      const out: Array<{ span: Span; depth: number }> = [];
      for (const s of idx.byId.values()) {
        if (errOnly && s.status !== "error") continue;
        if (q && !(searchText.get(s.id) ?? "").includes(q)) continue;
        out.push({ span: s, depth: 0 });
        if (out.length >= 500) break;
      }
      return out;
    }
    const out: Array<{ span: Span; depth: number }> = [];
    const walk = (list: Span[], depth: number) => {
      for (const s of list) {
        out.push({ span: s, depth });
        if (!closed.has(s.id)) walk(idx.children.get(s.id) ?? [], depth + 1);
      }
    };
    walk(idx.roots, 0);
    return out;
  }, [idx, closed, filtering, query, errOnly, searchText]);

  /* Reveal a span the user jumped to (error nav, search pick): every
     collapsed ancestor opens so the selection is actually visible. */
  const reveal = (id: string) => {
    onSetClosed((prev) => {
      const next = new Set(prev);
      let p = idx.parents.get(id);
      while (p) {
        next.delete(p);
        p = idx.parents.get(p);
      }
      return next;
    });
    onSelect(id);
  };

  const gotoError = (dir: 1 | -1) => {
    if (idx.errors.length === 0) return;
    const at = sel ? idx.errors.indexOf(sel) : -1;
    const ni = at === -1
      ? (dir === 1 ? 0 : idx.errors.length - 1)
      : (at + dir + idx.errors.length) % idx.errors.length;
    reveal(idx.errors[ni]);
  };

  const treeRef = useRef<HTMLDivElement | null>(null);
  const [scrollTop, setScrollTop] = useState(0);
  const [viewH, setViewH] = useState(760);
  useEffect(() => {
    const el = treeRef.current;
    if (!el || typeof ResizeObserver === "undefined") return;
    const update = () => setViewH(el.clientHeight || 760);
    update();
    const ro = new ResizeObserver(update);
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  /* Scroll a row index into the window — the windowed replacement for
     scrollIntoView, which can't target a row that isn't mounted. */
  const ensureVisible = (i: number) => {
    const el = treeRef.current;
    if (!el || i < 0) return;
    const top = PAD_TOP + i * ROW_H;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW_H > el.scrollTop + el.clientHeight)
      el.scrollTop = top + ROW_H - el.clientHeight;
  };

  /* Scroll to the selection only when the SELECTION changes (deep link,
     keyboard, click) — never when rows change identity, or expanding a
     chevron 800 rows away from the selection would yank the viewport back
     to it. No-op when already visible or hidden under a collapsed parent. */
  const lastSel = useRef<string | null>(null);
  useEffect(() => {
    if (sel === lastSel.current) return;
    lastSel.current = sel;
    ensureVisible(rows.findIndex((r) => r.span.id === sel));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sel, rows]);

  const clearFilters = () => {
    setQuery("");
    setErrOnly(false);
    // The row list changes identity wholesale when a filter clears — force
    // the selection effect to pull the selection back into the viewport.
    lastSel.current = null;
    treeRef.current?.focus({ preventScroll: true });
  };

  const { typeCounts, errorCount } = useMemo(() => {
    const counts: Array<[string, number]> = [];
    let errors = 0;
    for (const s of idx.byId.values()) {
      if (s.status === "error") errors++;
      const entry = counts.find(([t]) => t === s.type);
      if (entry) entry[1]++;
      else counts.push([s.type, 1]);
    }
    return { typeCounts: counts, errorCount: errors };
  }, [idx]);

  /* keyboard: ↑/↓ move through visible rows, ←/→ collapse/expand, e/E next
     and previous error, / focuses search. The handler lives on the scroll
     container (which is focusable — a focused row can unmount when it
     scrolls out of the window). */
  const onTreeKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const at = rows.findIndex((r) => r.span.id === sel);
      if (at === -1) {
        // In filter mode the selection is often not among the matches —
        // arrows should enter the list, not go dead. (Unfiltered, a
        // selection hidden under a collapsed ancestor stays put.)
        if (filtering && rows.length > 0) {
          onSelect(rows[0].span.id);
          ensureVisible(0);
          treeRef.current?.focus({ preventScroll: true });
        }
        return;
      }
      const ni = Math.min(rows.length - 1, Math.max(0, at + (e.key === "ArrowDown" ? 1 : -1)));
      const next = rows[ni];
      if (next) {
        onSelect(next.span.id);
        ensureVisible(ni);
        treeRef.current?.focus({ preventScroll: true });
      }
    } else if (e.key === "Escape" && filtering) {
      e.preventDefault();
      clearFilters();
    } else if ((e.key === "ArrowRight" || e.key === "ArrowLeft") && sel) {
      e.preventDefault();
      onSetClosed((prev) => {
        const next = new Set(prev);
        if (e.key === "ArrowRight") next.delete(sel);
        else next.add(sel);
        return next;
      });
    } else if (e.key === "e" || e.key === "E") {
      e.preventDefault();
      gotoError(e.shiftKey ? -1 : 1);
    } else if (e.key === "/") {
      e.preventDefault();
      searchRef.current?.focus();
    }
  };

  /* the window: which slice of rows is actually in the DOM. scrollTop is
     clamped so rows shrinking under a deep scroll (collapse-all) can't
     produce an empty slice while the browser catches up. */
  const top = Math.min(scrollTop, Math.max(0, rows.length * ROW_H - viewH));
  const first = Math.max(0, Math.floor((top - PAD_TOP) / ROW_H) - OVERSCAN);
  const last = Math.min(rows.length, Math.ceil((top - PAD_TOP + viewH) / ROW_H) + OVERSCAN);

  return (
    <div className="rv-tree">
      <div
        style={{
          display: "flex",
          justifyContent: "space-between",
          alignItems: "baseline",
          padding: "12px 12px 8px",
          borderBottom: `1px solid ${T.line}`,
        }}
      >
        <Eyebrow>trace</Eyebrow>
        <span
          style={{
            display: "inline-flex",
            alignItems: "center",
            gap: 10,
            fontFamily: T.mono,
            fontSize: 11,
            color: T.faint,
          }}
        >
          {typeCounts.map(([type, n]) => (
            <span
              key={type}
              title={`${n} ${type} span${n === 1 ? "" : "s"}`}
              style={{ display: "inline-flex", alignItems: "center", gap: 4 }}
            >
              <span
                style={{
                  width: 7,
                  height: 7,
                  borderRadius: 2,
                  background: HUES[type] ?? HUES.custom,
                }}
              />
              {n}
            </span>
          ))}
          {errorCount > 0 && (
            <span style={{ display: "inline-flex", alignItems: "center", gap: 3 }}>
              <button
                onClick={() => gotoError(-1)}
                aria-label="Previous error"
                title="Previous error (E)"
                style={errNavBtn}
              >
                ‹
              </button>
              <span style={{ color: T.error, fontWeight: 600 }} title={`${errorCount} error span${errorCount === 1 ? "" : "s"}`}>
                {errorCount} err
              </span>
              <button
                onClick={() => gotoError(1)}
                aria-label="Next error"
                title="Next error (e)"
                style={errNavBtn}
              >
                ›
              </button>
            </span>
          )}
        </span>
      </div>

      {/* search + filters — the flat, findable view over the same spans */}
      <div
        style={{
          display: "flex",
          gap: 6,
          alignItems: "center",
          padding: "8px 10px",
          borderBottom: `1px solid ${T.line}`,
        }}
      >
        <Search size={12} color={T.faint} style={{ flexShrink: 0 }} />
        <input
          ref={searchRef}
          value={query}
          onChange={(e) => setQuery(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Escape") {
              clearFilters();
            } else if (e.key === "Enter" && rows.length > 0) {
              reveal(rows[0].span.id);
              treeRef.current?.focus({ preventScroll: true });
            }
          }}
          placeholder="search spans and messages"
          aria-label="Search spans and messages"
          style={{
            flex: 1,
            minWidth: 0,
            border: "none",
            outline: "none",
            background: "transparent",
            fontFamily: T.mono,
            fontSize: 12,
            color: T.ink,
          }}
        />
        {errorCount > 0 && (
          <button
            onClick={() => setErrOnly((v) => !v)}
            aria-pressed={errOnly}
            title="Show only error spans"
            style={{
              border: `1px solid ${errOnly ? T.error : T.line}`,
              background: errOnly ? "var(--rv-errbg)" : T.panel,
              color: errOnly ? T.error : T.faint,
              borderRadius: 5,
              padding: "2px 7px",
              fontFamily: T.mono,
              fontSize: 10,
              cursor: "pointer",
            }}
          >
            errors
          </button>
        )}
        {!filtering && (
          <>
            <button
              onClick={() => {
                const all = new Set<string>();
                for (const id of idx.children.keys()) all.add(id);
                onSetClosed(all);
              }}
              title="Collapse all"
              style={errNavBtn}
            >
              −
            </button>
            <button onClick={() => onSetClosed(new Set())} title="Expand all" style={errNavBtn}>
              +
            </button>
          </>
        )}
      </div>
      {filtering && (
        <div
          style={{
            padding: "4px 12px",
            fontFamily: T.mono,
            fontSize: 10.5,
            color: T.faint,
            borderBottom: `1px solid ${T.line}`,
          }}
        >
          {rows.length === 500 ? "500+" : rows.length} match{rows.length === 1 ? "" : "es"} ·{" "}
          <button
            onClick={clearFilters}
            style={{ border: "none", background: "none", padding: 0, font: "inherit", color: T.signal, cursor: "pointer", textDecoration: "underline" }}
          >
            clear
          </button>{" "}
          (esc)
        </div>
      )}

      {/* Windowed: rows are fixed-height, so visibility is arithmetic.
          A spacer keeps the scrollbar honest; only [first, last) mount. */}
      <div
        ref={treeRef}
        tabIndex={0}
        role="tree"
        aria-label="Trace spans"
        // The windowed rows unmount when scrolled away, so the container
        // holds focus and points assistive tech at the active row by id.
        aria-activedescendant={sel ? `rv-row-${sel}` : undefined}
        onKeyDown={onTreeKey}
        onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
        style={{ padding: `${PAD_TOP}px 0`, overflowY: "auto", maxHeight: "min(72vh, 760px)" }}
      >
        <div style={{ height: rows.length * ROW_H, position: "relative" }}>
          <div style={{ position: "absolute", top: first * ROW_H, left: 0, right: 0 }}>
            {rows.slice(first, last).map(({ span, depth }, wi) => (
              <TreeRow
                key={span.id}
                span={span}
                depth={depth}
                idx={idx}
                posinset={first + wi + 1}
                setsize={rows.length}
                selected={sel === span.id}
                linked={linkedId === span.id}
                hasKids={!filtering && (idx.children.get(span.id) ?? []).length > 0}
                open={!closed.has(span.id)}
                onToggle={() => {
                  onSetClosed((prev) => {
                    const next = new Set(prev);
                    if (next.has(span.id)) next.delete(span.id);
                    else next.add(span.id);
                    return next;
                  });
                  // Keep focus on the keyboard surface: a click focuses the
                  // row/chevron, which may unmount when it scrolls out of
                  // the window — stranding focus on <body> and killing
                  // arrow-key nav.
                  treeRef.current?.focus({ preventScroll: true });
                }}
                onSelect={() => {
                  if (filtering) reveal(span.id);
                  else onSelect(span.id);
                  treeRef.current?.focus({ preventScroll: true });
                }}
              />
            ))}
          </div>
        </div>
      </div>

      {/* the keyboard legend — persistent, quiet, discoverable */}
      <div
        style={{
          padding: "6px 12px",
          borderTop: `1px solid ${T.line}`,
          fontFamily: T.mono,
          fontSize: 10,
          color: T.faint,
          letterSpacing: "0.04em",
        }}
      >
        ↑↓ rows · ←→ fold · e / E next / prev error · / search · esc clear
      </div>
    </div>
  );
}

const errNavBtn: React.CSSProperties = {
  border: `1px solid ${T.line}`,
  background: T.panel,
  color: T.faint,
  borderRadius: 5,
  width: 18,
  height: 18,
  display: "inline-flex",
  alignItems: "center",
  justifyContent: "center",
  fontFamily: T.mono,
  fontSize: 12,
  cursor: "pointer",
  padding: 0,
};

/* ------------------------------------------------------- transcript flow */

type FlowBlock =
  | { key: string; kind: "boundary"; spanId: string; label: string }
  | { key: string; kind: "msg"; spanId: string; msg: Message; err?: string };

/**
 * The session as a conversation: every llm_call's messages in document
 * order. Full-history inputs (an exact proxy capture resends the whole
 * conversation each call) collapse via POSITIONAL prefix-echo matching —
 * input[i] is skipped only while it matches the i-th message already
 * rendered, so a genuinely repeated turn ("continue" twice) or a turn
 * sharing a long prefix with another still renders; a replayed history
 * never does. Delta-style inputs (reconstructed imports) pass through
 * untouched. Subagent spans reset the echo cursor (their histories are
 * their own) and become section boundaries; errored calls that add no
 * new messages still surface their error as a block.
 */
function buildFlow(run: Run, idx: RunIndex): FlowBlock[] {
  const blocks: FlowBlock[] = [];
  // Length + head: collision-safe in practice without hashing megabytes.
  const fp = (m: Message) => {
    const str = stringify(m.content);
    return `${m.role}|${str.length}|${str.slice(0, 600)}`;
  };
  let emitted: string[] = []; // fingerprints of rendered msgs, in order
  for (const s of run.spans) {
    if (s.type === "agent" && s.parent_id && idx.byId.has(s.parent_id)) {
      blocks.push({ key: `b:${s.id}`, kind: "boundary", spanId: s.id, label: spanLabel(s) });
      emitted = []; // a subagent's history is its own conversation
      continue;
    }
    const err = s.status === "error" ? String(s.error?.message ?? "error") : undefined;
    // Trailing user messages (importers store them as custom spans with
    // messages) belong in the transcript too — read any span carrying
    // input.messages, not just llm_calls.
    const input = (s as { input?: { messages?: Message[] } }).input?.messages ?? [];
    const output = (s as { output?: { messages?: Message[] } }).output?.messages ?? [];
    if (s.type !== "llm_call" && input.length === 0 && output.length === 0) continue;

    let errPending = err;
    const push = (msg: Message, f: string) => {
      blocks.push({ key: `${s.id}:${blocks.length}`, kind: "msg", spanId: s.id, msg, err: errPending });
      emitted.push(f);
      errPending = undefined;
    };
    let echo = 0; // how far input replays what's already on screen
    for (let i = 0; i < input.length; i++) {
      const f = fp(input[i]);
      if (i === echo && echo < emitted.length && f === emitted[echo]) {
        echo++;
        continue;
      }
      push(input[i], f);
    }
    for (const m of output) push(m, fp(m));
    if (errPending) {
      // The call added nothing new (a failed retry) — the error must not
      // vanish with it.
      blocks.push({
        key: `${s.id}:err`,
        kind: "msg",
        spanId: s.id,
        msg: { role: "system", content: [] } as unknown as Message,
        err: errPending,
      });
    }
  }
  return blocks;
}

const FLOW_PAGE = 120;

function FlowView({
  run,
  idx,
  onInspect,
}: {
  run: Run;
  idx: RunIndex;
  onInspect: (spanId: string) => void;
}) {
  const blocks = useMemo(() => buildFlow(run, idx), [run, idx]);
  const [shown, setShown] = useState(FLOW_PAGE);

  if (blocks.length === 0) {
    return (
      <div style={{ fontFamily: T.mono, fontSize: 12, color: T.faint, padding: 32, textAlign: "center" }}>
        nothing conversational to read here — the tree view has the spans
      </div>
    );
  }
  return (
    <div style={{ maxWidth: 780, margin: "0 auto", padding: "8px 20px 24px" }}>
      {blocks.slice(0, shown).map((b) =>
        b.kind === "boundary" ? (
          <div
            key={b.key}
            onClick={() => onInspect(b.spanId)}
            title="inspect in tree"
            style={{
              display: "flex",
              alignItems: "center",
              gap: 8,
              margin: "20px 0 4px",
              cursor: "pointer",
              fontFamily: T.mono,
              fontSize: 11,
              letterSpacing: "0.12em",
              textTransform: "uppercase",
              color: HUES.agent,
            }}
          >
            <Bot size={12} /> {b.label}
            <span style={{ flex: 1, borderTop: `1px solid ${T.line}` }} />
          </div>
        ) : (
          <div
            key={b.key}
            className="rv-flowblock"
            onClick={(e) => {
              // Reading is the job — only the gutter click inspects, so
              // text selection and link clicks stay undisturbed.
              if ((e.target as HTMLElement).closest("a,button,summary,input")) return;
              if (window.getSelection()?.toString()) return;
              onInspect(b.spanId);
            }}
            title="click to inspect this turn in the tree"
          >
            {b.err && (
              <div
                style={{
                  border: `1px solid ${T.error}`,
                  background: "var(--rv-errbg)",
                  color: T.error,
                  borderRadius: 6,
                  padding: "8px 12px",
                  fontFamily: T.mono,
                  fontSize: 12,
                  margin: "12px 0 0",
                }}
              >
                {b.err}
              </div>
            )}
            <MessageView msg={b.msg} />
          </div>
        ),
      )}
      {blocks.length > shown && (
        <button
          onClick={() => setShown((n) => n + FLOW_PAGE)}
          style={{
            display: "block",
            margin: "16px auto 0",
            border: `1px solid ${T.line}`,
            background: T.panel,
            borderRadius: 6,
            padding: "6px 14px",
            fontFamily: T.mono,
            fontSize: 11,
            cursor: "pointer",
            color: T.faint,
          }}
        >
          show {Math.min(FLOW_PAGE, blocks.length - shown)} more of {fmtInt(blocks.length - shown)} remaining
        </button>
      )}
    </div>
  );
}

/** Where a fresh page should land: the first error if there is one, else
 *  the first substantive turn, else the root — never an empty root panel. */
const smartDefault = (idx: RunIndex): string | null =>
  idx.errors[0] ?? idx.llmCalls[0] ?? idx.roots[0]?.id ?? null;

function LoadedViewer({ run }: { run: Run }) {
  const idx = useMemo(() => indexRun(run), [run]);
  const [sel, setSel] = useState<string | null>(() => smartDefault(idx));
  const [closed, setClosed] = useState<Set<string>>(new Set());
  const [raw, setRaw] = useState(false);
  const [copied, setCopied] = useState(false);
  /* Transcript is the default for agent sessions — the audience reads
     first (deep links flip to the tree below, where the span lives). */
  const [mode, setMode] = useState<"transcript" | "tree">(() =>
    idx.llmCalls.length > 0 && idx.roots[0]?.type === "agent" ? "transcript" : "tree",
  );
  const [linked, setLinked] = useState<string | null>(null);

  /* deep link: read #span=<id> on mount, write it on select. TreeColumn's
     selection effect scrolls it into the window once it renders; the row
     pulses so the reader sees WHERE the sharer pointed them.
     useLayoutEffect so the flip to tree happens before paint — a deep-link
     arrival must not flash a page of transcript first. (SSR renders the
     deterministic default; the layout effect corrects it pre-paint on the
     client, so there's no hydration mismatch and no visible flash.) */
  useLayoutEffect(() => {
    try {
      const m = window.location.hash.match(/span=([\w.-]+)/);
      if (m && idx.byId.has(m[1])) {
        setSel(m[1]);
        setMode("tree");
        setLinked(m[1]);
        const t = window.setTimeout(() => setLinked(null), 3200);
        return () => window.clearTimeout(t);
      }
    } catch {
      /* sandboxed embeds may not expose location — fine */
    }
  }, [idx]);

  const select = (id: string) => {
    setSel(id);
    setCopied(false);
    try {
      history.replaceState(null, "", `#span=${id}`);
    } catch {
      /* ignore in sandboxes */
    }
  };

  const copyLink = () => {
    if (!sel) return;
    let text = `#span=${sel}`;
    try {
      const u = new URL(window.location.href);
      u.hash = `span=${sel}`;
      text = u.toString();
    } catch {
      /* keep relative anchor */
    }
    navigator.clipboard?.writeText(text).catch(() => {});
    setCopied(true);
    window.setTimeout(() => setCopied(false), 1400);
  };

  const selected = sel ? idx.byId.get(sel) : undefined;
  const created = new Date(run.created_at);

  return (
    <div className="rv" style={{ fontFamily: T.sans, color: T.ink }}>
      {/* selection announced for screen readers — the visual tree handles
          sighted users; this handles everyone else */}
      <div aria-live="polite" className="rv-visually-hidden">
        {selected ? `${spanLabel(selected)} · ${String(selected.type)}` : ""}
      </div>

      {/* ------------------------------------------------------- header */}
      <Eyebrow>
        run · {run.source?.label ?? run.source?.kind ?? "upload"} ·{" "}
        {created.toISOString().slice(0, 10)}
        {run.group?.item ? ` · item ${run.group.item}` : ""}
      </Eyebrow>
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          alignItems: "flex-end",
          gap: 20,
          margin: "8px 0 16px",
        }}
      >
        <h1
          style={{
            fontFamily: T.serif,
            fontSize: 28,
            fontWeight: 500,
            margin: 0,
            flex: "1 1 320px",
            letterSpacing: "-0.01em",
          }}
        >
          {run.name ?? "Untitled session"}
        </h1>
        <div style={{ display: "flex", gap: 24 }}>
          {idx.tokensIn != null && (
            <Metric label="tokens in" value={fmtInt(idx.tokensIn)} />
          )}
          {idx.tokensOut != null && (
            <Metric label="tokens out" value={fmtInt(idx.tokensOut)} />
          )}
          {idx.tokensCached != null && idx.tokensCached > 0 && (
            <Metric label="cached" value={fmtInt(idx.tokensCached)} />
          )}
          {idx.cost != null && <Metric label="cost" value={fmtCost(idx.cost)} />}
          {idx.durMs != null && (
            <Metric label="duration" value={fmtDur(idx.durMs)} />
          )}
        </div>
      </div>

      {(idx.models.length > 0 ||
        run.source?.fidelity != null ||
        (run.tags?.length ?? 0) > 0 ||
        (run.scores?.length ?? 0) > 0) && (
      <div
        style={{
          display: "flex",
          flexWrap: "wrap",
          gap: 8,
          alignItems: "center",
          marginBottom: 16,
        }}
      >
        {run.source?.fidelity != null && (
          <Chip
            title={
              run.source.fidelity === "exact"
                ? "Recorded on the wire — verbatim requests and responses"
                : run.source.fidelity === "reconstructed"
                  ? "Rebuilt from the agent's own transcript — messages and usage, not raw wire bytes"
                  : `Capture fidelity: ${String(run.source.fidelity)}`
            }
          >
            {run.source.fidelity === "exact"
              ? "exact capture"
              : run.source.fidelity === "reconstructed"
                ? "reconstructed transcript"
                : String(run.source.fidelity)}
          </Chip>
        )}
        {idx.models.map((m) => (
          <Chip key={m}>{m}</Chip>
        ))}
        {(run.tags ?? []).map((t) => (
          <span
            key={t}
            style={{ fontFamily: T.mono, fontSize: 12, color: T.faint }}
          >
            #{t}
          </span>
        ))}
        {(run.scores ?? []).map((s) => (
          <Chip key={s.name} title={s.comment}>
            <span style={{ color: T.faint }}>{s.name}</span>
            <span
              style={{
                color:
                  s.value === true
                    ? T.signal
                    : s.value === false
                      ? T.error
                      : T.ink,
                fontWeight: 600,
              }}
            >
              {s.value === true ? "✓" : s.value === false ? "✕" : String(s.value)}
            </span>
          </Chip>
        ))}
      </div>
      )}

      {/* ------------------------------------------------- mode toggle */}
      <div style={{ display: "flex", justifyContent: "flex-end", marginBottom: 8 }}>
        <div
          role="group"
          aria-label="Viewer mode"
          style={{
            display: "inline-flex",
            border: `1px solid ${T.line}`,
            borderRadius: 6,
            overflow: "hidden",
          }}
        >
          {(["transcript", "tree"] as const).map((m) => (
            <button
              key={m}
              onClick={() => setMode(m)}
              aria-pressed={mode === m}
              style={{
                border: "none",
                padding: "4px 12px",
                fontFamily: T.mono,
                fontSize: 11,
                cursor: "pointer",
                background: mode === m ? T.ink : T.panel,
                color: mode === m ? T.paper : T.faint,
              }}
            >
              {m}
            </button>
          ))}
        </div>
      </div>

      {mode === "transcript" ? (
        <div className="rv-cols" style={{ display: "block" }}>
          <FlowView
            run={run}
            idx={idx}
            onInspect={(id) => {
              select(id);
              setMode("tree");
            }}
          />
        </div>
      ) : (
      /* ---------------------------------------------------- two panels */
      <div className="rv-cols">
        <TreeColumn
          idx={idx}
          sel={sel}
          closed={closed}
          linkedId={linked}
          onSelect={select}
          onSetClosed={setClosed}
        />

        <div style={{ flex: 1, minWidth: 0, padding: "16px 20px 24px" }}>
          {selected ? (
            <>
              <div
                style={{
                  display: "flex",
                  flexWrap: "wrap",
                  alignItems: "center",
                  gap: 10,
                  paddingBottom: 12,
                  borderBottom: `1px solid ${T.line}`,
                  marginBottom: 14,
                }}
              >
                <span
                  style={{
                    width: 9,
                    height: 9,
                    borderRadius: 2,
                    background: spanHue(selected),
                  }}
                />
                <span style={{ fontSize: 15, fontWeight: 600 }}>
                  {spanLabel(selected)}
                </span>
                <span
                  style={{ fontFamily: T.mono, fontSize: 11, color: T.faint }}
                >
                  {String(selected.type)}
                </span>
                {Number.isFinite(spanDur(selected)) && (
                  <span
                    title={
                      selected.started_at
                        ? `${new Date(selected.started_at).toLocaleTimeString()} → ${selected.ended_at ? new Date(selected.ended_at).toLocaleTimeString() : "…"}`
                        : undefined
                    }
                    style={{ fontFamily: T.mono, fontSize: 11, color: T.faint }}
                  >
                    · {fmtDur(spanDur(selected))}
                  </span>
                )}
                {selected.fidelity && (
                  <span
                    title="Capture fidelity for this span"
                    style={{
                      fontFamily: T.mono,
                      fontSize: 10,
                      letterSpacing: "0.08em",
                      textTransform: "uppercase",
                      color: T.faint,
                      border: `1px solid ${T.line}`,
                      borderRadius: 999,
                      padding: "2px 8px",
                    }}
                  >
                    {String(selected.fidelity)}
                  </span>
                )}
                <span style={{ flex: 1 }} />
                <button
                  onClick={copyLink}
                  aria-label="Copy link to this span"
                  style={{
                    display: "inline-flex",
                    alignItems: "center",
                    gap: 5,
                    border: `1px solid ${T.line}`,
                    background: T.panel,
                    borderRadius: 6,
                    padding: "4px 9px",
                    fontFamily: T.mono,
                    fontSize: 11,
                    cursor: "pointer",
                    color: copied ? T.signal : T.ink,
                  }}
                >
                  {copied ? <Check size={12} /> : <Copy size={12} />}
                  {copied ? "copied" : "copy link"}
                </button>
                <div
                  role="group"
                  aria-label="View mode"
                  style={{
                    display: "inline-flex",
                    border: `1px solid ${T.line}`,
                    borderRadius: 6,
                    overflow: "hidden",
                  }}
                >
                  {(["formatted", "raw"] as const).map((mode) => (
                    <button
                      key={mode}
                      onClick={() => setRaw(mode === "raw")}
                      aria-pressed={raw === (mode === "raw")}
                      style={{
                        border: "none",
                        padding: "4px 10px",
                        fontFamily: T.mono,
                        fontSize: 11,
                        cursor: "pointer",
                        background:
                          raw === (mode === "raw") ? T.ink : T.panel,
                        color: raw === (mode === "raw") ? T.paper : T.faint,
                      }}
                    >
                      {mode}
                    </button>
                  ))}
                </div>
              </div>

              {selected.status === "error" && selected.error && (
                <div
                  style={{
                    border: `1px solid ${T.error}`,
                    background: "rgba(179,64,46,0.06)",
                    color: T.error,
                    borderRadius: 6,
                    padding: "8px 12px",
                    fontFamily: T.mono,
                    fontSize: 12,
                    marginBottom: 14,
                  }}
                >
                  {selected.error.type ? `${selected.error.type}: ` : ""}
                  {String(selected.error.message ?? "")}
                </div>
              )}

              {raw ? (
                /* Keyed like the Boundary below: without the key, this
                   JsonBlock reconciles in place across selections and the
                   clamp expander's showAll state leaks — span B would mount
                   with span A's expansion. */
                <JsonBlock key={sel ?? ""} value={selected} tall />
              ) : (
                /* Keyed by span: a new selection resets both a caught error
                   and any expander state left inside the old detail. The raw
                   toggle lives in the header above, so it stays usable when
                   this subtree falls back. */
                <Boundary
                  key={sel ?? ""}
                  fallback={(message) => <SpanFallback message={message} />}
                >
                  <SpanDetail span={selected} />
                  {selected.metadata && Object.keys(selected.metadata).length > 0 && (
                    <>
                      <SectionLabel>metadata</SectionLabel>
                      <JsonBlock value={selected.metadata} />
                    </>
                  )}
                </Boundary>
              )}
            </>
          ) : (
            <div style={{ color: T.faint, fontSize: 13, padding: 24, lineHeight: 2 }}>
              Select a span to inspect it.
              <br />
              <span style={{ fontSize: 12 }}>
                ↑↓ move through the trace · ←→ collapse and expand
              </span>
            </div>
          )}
        </div>
      </div>
      )}
    </div>
  );
}
