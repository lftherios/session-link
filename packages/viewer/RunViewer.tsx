"use client";

import { useEffect, useMemo, useRef, useState } from "react";
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

const T = {
  paper: "#f5f6f3",
  panel: "#fdfdfb",
  soft: "#eef1ec",
  ink: "#17201c",
  faint: "#5b6660",
  line: "#d8ddd7",
  signal: "#0e6f5c",
  error: "#b3402e",
  serif:
    '"Iowan Old Style","Palatino Linotype",Palatino,"Book Antiqua",Georgia,serif',
  sans: 'system-ui,-apple-system,"Segoe UI",Roboto,sans-serif',
  mono: 'ui-monospace,"SF Mono","Cascadia Code",Menlo,Consolas,monospace',
};

// Record<string, ...> on purpose: the wire format is open, so spans with
// types this build doesn't know can arrive — they take the `custom` hue.
const HUES: Record<string, string> = {
  llm_call: T.signal,
  tool_call: "#a16418",
  retrieval: "#28629c",
  agent: "#5b4b8a",
  custom: "#5b6660",
};

const spanHue = (s: Span) =>
  s.status === "error" ? T.error : (HUES[s.type] ?? HUES.custom);

/* --------------------------------------------------------------- helpers */

const ts = (s?: string) => (s ? new Date(s).getTime() : NaN);

function fmtDur(msVal: number): string {
  if (!Number.isFinite(msVal)) return "";
  if (msVal < 1000) return `${Math.round(msVal)}ms`;
  return `${(msVal / 1000).toFixed(2)}s`;
}

const fmtInt = (n: number) => n.toLocaleString("en-US");

const fmtCost = (n: number) => (n < 0.01 ? `$${n.toFixed(4)}` : `$${n.toFixed(2)}`);

const spanDur = (s: Span) => ts(s.ended_at) - ts(s.started_at);

const spanLabel = (s: Span) => s.name || s.type;

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
  t0: number;
  t1: number;
  models: string[];
  tokensIn?: number;
  tokensOut?: number;
  cost?: number;
  durMs?: number;
}

function indexRun(run: Run): RunIndex {
  const byId = new Map<string, Span>();
  for (const s of run.spans) byId.set(s.id, s);

  const children = new Map<string, Span[]>();
  const roots: Span[] = [];
  for (const s of run.spans) {
    const pid = s.parent_id ?? null;
    if (pid && byId.has(pid)) {
      const list = children.get(pid) ?? [];
      list.push(s);
      children.set(pid, list);
    } else {
      roots.push(s);
    }
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
  let cost = 0;
  let sawUsage = false;
  let sawCost = false;
  for (const s of run.spans) {
    if (s.type !== "llm_call") continue;
    const label = `${s.model.provider ? s.model.provider + "/" : ""}${s.model.id}`;
    if (!models.includes(label)) models.push(label);
    if (s.usage) {
      sawUsage = true;
      tin += s.usage.input_tokens ?? 0;
      tout += s.usage.output_tokens ?? 0;
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
    t0,
    t1,
    models,
    tokensIn: run.metrics?.input_tokens ?? (sawUsage ? tin : undefined),
    tokensOut: run.metrics?.output_tokens ?? (sawUsage ? tout : undefined),
    cost: run.metrics?.cost_usd ?? (sawCost ? cost : undefined),
    durMs: run.metrics?.latency_ms ?? (hasTimeline ? t1 - t0 : undefined),
  };
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

function JsonBlock({ value, tall = false }: { value: unknown; tall?: boolean }) {
  return (
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
      {stringify(value)}
    </pre>
  );
}

/* ----------------------------------------------------- message rendering */

function PartView({ part }: { part: ContentPart }) {
  switch (part.type) {
    case "text":
      return (
        <div style={{ fontSize: 13.5, lineHeight: 1.6, whiteSpace: "pre-wrap" }}>
          {part.text}
        </div>
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
            {part.text}
          </div>
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

function MessageView({ msg }: { msg: Message }) {
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
  selected,
  hasKids,
  open,
  onToggle,
  onSelect,
}: {
  span: Span;
  depth: number;
  idx: RunIndex;
  selected: boolean;
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

  return (
    <div
      id={`rv-row-${span.id}`}
      role="button"
      tabIndex={0}
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
      className="rv-row"
      style={{
        display: "flex",
        alignItems: "center",
        gap: 6,
        height: ROW_H,
        padding: "0 12px 0 0",
        paddingLeft: 10 + depth * 14,
        cursor: "pointer",
        background: selected ? "rgba(14,111,92,0.08)" : "transparent",
        boxShadow: selected ? `inset 2px 0 0 ${T.signal}` : "none",
      }}
    >
      {hasKids ? (
        <button
          aria-label={open ? "Collapse" : "Expand"}
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
        {spanLabel(span)}
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
 */
export function RunViewer({ run, src }: { run?: Run; src?: string }) {
  const [fetched, setFetched] = useState<Run | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);

  useEffect(() => {
    if (run || !src) return;
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
  return <LoadedViewer run={resolved} />;
}

/** Every visible tree row is exactly this tall — the invariant the
 *  windowing math depends on, enforced by an explicit height on the row. */
const ROW_H = 30;
const OVERSCAN = 12;

function LoadedViewer({ run }: { run: Run }) {
  const idx = useMemo(() => indexRun(run), [run]);
  const [sel, setSel] = useState<string | null>(idx.roots[0]?.id ?? null);
  const [closed, setClosed] = useState<Set<string>>(new Set());
  const [raw, setRaw] = useState(false);
  const [copied, setCopied] = useState(false);

  /* The visible tree, flattened. Thousands of rows are normal for real
     agent traces, so only the window around the scroll position mounts —
     this array is cheap (plain objects), the DOM is what's rationed. */
  const rows = useMemo(() => {
    const out: Array<{ span: Span; depth: number }> = [];
    const walk = (list: Span[], depth: number) => {
      for (const s of list) {
        out.push({ span: s, depth });
        if (!closed.has(s.id)) walk(idx.children.get(s.id) ?? [], depth + 1);
      }
    };
    walk(idx.roots, 0);
    return out;
  }, [idx, closed]);

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
    const top = i * ROW_H;
    if (top < el.scrollTop) el.scrollTop = top;
    else if (top + ROW_H > el.scrollTop + el.clientHeight)
      el.scrollTop = top + ROW_H - el.clientHeight;
  };

  /* deep link: read #span=<id> on mount, write it on select */
  useEffect(() => {
    try {
      const m = window.location.hash.match(/span=([\w.-]+)/);
      if (m && idx.byId.has(m[1])) {
        setSel(m[1]);
        ensureVisible(rows.findIndex((r) => r.span.id === m[1]));
      }
    } catch {
      /* sandboxed embeds may not expose location — fine */
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
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

  const { typeCounts, errorCount } = useMemo(() => {
    const counts: Array<[string, number]> = [];
    let errors = 0;
    for (const s of run.spans) {
      if (s.status === "error") errors++;
      const entry = counts.find(([t]) => t === s.type);
      if (entry) entry[1]++;
      else counts.push([s.type, 1]);
    }
    return { typeCounts: counts, errorCount: errors };
  }, [run]);

  /* keyboard: ↑/↓ move through visible rows, ←/→ collapse/expand. The
     handler lives on the scroll container (which is focusable — a focused
     row can unmount when it scrolls out of the window). */
  const onTreeKey = (e: React.KeyboardEvent) => {
    if (e.key === "ArrowDown" || e.key === "ArrowUp") {
      e.preventDefault();
      const at = rows.findIndex((r) => r.span.id === sel);
      const ni = Math.min(rows.length - 1, Math.max(0, at + (e.key === "ArrowDown" ? 1 : -1)));
      const next = rows[ni];
      if (next) {
        select(next.span.id);
        ensureVisible(ni);
        treeRef.current?.focus();
      }
    } else if ((e.key === "ArrowRight" || e.key === "ArrowLeft") && sel) {
      e.preventDefault();
      setClosed((prev) => {
        const next = new Set(prev);
        if (e.key === "ArrowRight") next.delete(sel);
        else next.add(sel);
        return next;
      });
    }
  };

  /* the window: which slice of rows is actually in the DOM */
  const first = Math.max(0, Math.floor(scrollTop / ROW_H) - OVERSCAN);
  const last = Math.min(rows.length, Math.ceil((scrollTop + viewH) / ROW_H) + OVERSCAN);

  return (
    <div className="rv" style={{ fontFamily: T.sans, color: T.ink }}>
      <style>{`
        .rv *{box-sizing:border-box}
        .rv button{font:inherit}
        .rv pre{margin:0}
        .rv ::selection{background:rgba(14,111,92,.18)}
        .rv .rv-row:hover{background:rgba(23,32,28,.045)}
        .rv :where(button,[role=button]):focus-visible{outline:2px solid ${T.signal};outline-offset:2px;border-radius:4px}
        .rv .rv-cols{display:flex;border:1px solid ${T.line};border-radius:10px;background:${T.panel};overflow:hidden}
        .rv .rv-tree{width:320px;flex-shrink:0;border-right:1px solid ${T.line}}
        @media(max-width:880px){
          .rv .rv-cols{flex-direction:column}
          .rv .rv-tree{width:100%;border-right:none;border-bottom:1px solid ${T.line}}
        }
        @media(prefers-reduced-motion:no-preference){.rv .rv-row{transition:background .12s ease}}
      `}</style>

      {/* ------------------------------------------------------- header */}
      <Eyebrow>
        run · {run.source?.label ?? run.source?.kind ?? "upload"} ·{" "}
        {created.toISOString().slice(0, 10)}
        {run.group?.item ? ` · item ${run.group.item}` : ""}
        {run.source?.fidelity ? ` · ${run.source.fidelity}` : ""}
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
          {idx.cost != null && <Metric label="cost" value={fmtCost(idx.cost)} />}
          {idx.durMs != null && (
            <Metric label="duration" value={fmtDur(idx.durMs)} />
          )}
        </div>
      </div>

      {(idx.models.length > 0 ||
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

      {/* ---------------------------------------------------- two panels */}
      <div className="rv-cols">
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
                <span style={{ color: T.error, fontWeight: 600 }} title={`${errorCount} error span${errorCount === 1 ? "" : "s"}`}>
                  {errorCount} err
                </span>
              )}
            </span>
          </div>
          {/* Windowed: rows are fixed-height, so visibility is arithmetic.
              A spacer keeps the scrollbar honest; only [first, last) mount. */}
          <div
            ref={treeRef}
            tabIndex={0}
            aria-label="Trace spans"
            onKeyDown={onTreeKey}
            onScroll={(e) => setScrollTop(e.currentTarget.scrollTop)}
            style={{ padding: "6px 0", overflowY: "auto", maxHeight: "min(72vh, 760px)" }}
          >
            <div style={{ height: rows.length * ROW_H, position: "relative" }}>
              <div style={{ position: "absolute", top: first * ROW_H, left: 0, right: 0 }}>
                {rows.slice(first, last).map(({ span, depth }) => (
                  <TreeRow
                    key={span.id}
                    span={span}
                    depth={depth}
                    idx={idx}
                    selected={sel === span.id}
                    hasKids={(idx.children.get(span.id) ?? []).length > 0}
                    open={!closed.has(span.id)}
                    onToggle={() =>
                      setClosed((prev) => {
                        const next = new Set(prev);
                        if (next.has(span.id)) next.delete(span.id);
                        else next.add(span.id);
                        return next;
                      })
                    }
                    onSelect={() => select(span.id)}
                  />
                ))}
              </div>
            </div>
          </div>
        </div>

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
                  {selected.type}
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
                    {selected.fidelity}
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
                  {selected.error.message}
                </div>
              )}

              {raw ? (
                <JsonBlock value={selected} tall />
              ) : (
                <>
                  <SpanDetail span={selected} />
                  {selected.metadata && Object.keys(selected.metadata).length > 0 && (
                    <>
                      <SectionLabel>metadata</SectionLabel>
                      <JsonBlock value={selected.metadata} />
                    </>
                  )}
                </>
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
    </div>
  );
}
