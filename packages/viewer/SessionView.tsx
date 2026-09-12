import { useEffect, useLayoutEffect, useMemo, useRef, useState, type MouseEvent as ReactMouseEvent, type ReactNode } from "react";
import { Search, ChevronDown, ChevronLeft, ChevronRight, ArrowLeft, Share2, Pencil, FileJson, MessageSquare, Copy, ListTree } from "lucide-react";
import { ViewDialog } from "./ViewDialog";
import { withAnchor } from "./encryption";
import type { ViewDraft } from "./view-draft";
import type { ContentPart, Run, Span } from "@session-link/format";
import { buildFlow, defaultExchange, duration, elapsed, eventTime, exchangesFor, harnessName, messageText, previewText, promptLabel, promptText, partUnit, readerFlow, readingKey, readingRole, reasoningUnavailable, responseFor, selectionPrefixes, sessionLabel, sessionTitle, shortText, sourceRange, type Exchange, type MessageBlock } from "./session-model";

export type LocalViewer = { source: string; project?: string; title?: string };
type Passage = { unit: string; start: number; end: number };
export type SessionStats = { tokensIn?: number; tokensOut?: number; cost?: number; durMs?: number; models: string[]; errors: number; modelCalls: number; toolCalls: number };
type Fact = { label: string; value: string; note?: string; title?: string; wide?: boolean; mono?: boolean; error?: boolean };
type Props = { run: Run; local?: LocalViewer; renderPart: (part: ContentPart, full?: boolean) => ReactNode; renderSpan: (span: Span) => ReactNode; renderTree: () => ReactNode; stats: SessionStats };
const CSS = `
.sv{color:var(--rv-ink);font-family:system-ui,sans-serif}.sv :where(button,input,textarea,a){font:inherit}.sv *{box-sizing:border-box}
.sv button,.sv .sv-button{display:inline-flex;align-items:center;justify-content:center;gap:6px;border:1px solid var(--rv-line);background:var(--rv-panel);color:var(--rv-ink);border-radius:7px;padding:8px 12px;font-size:12px;cursor:pointer;text-decoration:none}
.sv button:disabled{opacity:.4;cursor:default}.sv button.sv-quiet{border-color:transparent;background:transparent;color:var(--rv-faint);padding:6px 8px}.sv button:hover:not(:disabled){background:var(--rv-soft)}
.sv button.sv-primary{background:var(--rv-signal);color:var(--rv-paper);border-color:var(--rv-signal)}.sv button.sv-primary:hover:not(:disabled){filter:brightness(.93)}
.sv :where(button,input,textarea,a,summary):focus-visible{outline:2px solid var(--rv-signal);outline-offset:3px}
.sv .sv-meta{font:11px/1.65 ui-monospace,monospace;color:var(--rv-faint)}
.sv .sv-header{display:flex;align-items:flex-start;gap:14px;margin:4px 0 30px}.sv .sv-back{flex:none;display:inline-flex;align-items:center;justify-content:center;width:36px;height:36px;border:1px solid transparent;border-radius:8px;color:var(--rv-faint);text-decoration:none}.sv .sv-back:hover{background:var(--rv-soft);color:var(--rv-ink)}
.sv .sv-heading{flex:1;min-width:0;padding-top:1px}.sv h1{display:inline;font:500 30px/1.2 "Iowan Old Style",Palatino,Georgia,serif;letter-spacing:-.025em;overflow-wrap:anywhere;margin:0;border-radius:4px;box-decoration-break:clone;-webkit-box-decoration-break:clone}.sv .sv-untitled{color:var(--rv-faint);font-weight:400}.sv .sv-local h1{cursor:text}.sv .sv-local h1:hover{background:var(--rv-soft);box-shadow:0 0 0 5px var(--rv-soft)}
.sv .sv-edit-title{vertical-align:middle;margin-left:6px;padding:5px!important;opacity:0;transition:opacity .12s}.sv .sv-heading:hover .sv-edit-title,.sv .sv-heading:focus-within .sv-edit-title,.sv .sv-edit-title:focus-visible{opacity:1}.sv .sv-title-status{display:block;margin-top:6px}
.sv .sv-share{flex:none;margin-left:auto;padding:11px 16px;font-size:13px;font-weight:550}
.sv input,.sv textarea{border:1px solid var(--rv-line);border-radius:6px;background:var(--rv-panel);color:var(--rv-ink);padding:10px 12px;min-width:0}.sv .sv-title-input{display:block;width:100%;font:500 30px/1.2 "Iowan Old Style",Palatino,Georgia,serif;letter-spacing:-.025em;padding:0 0 2px;border:0;border-bottom:2px solid var(--rv-signal);border-radius:0;background:transparent;outline:none}.sv .sv-title-input:focus-visible{outline:none}.sv .sv-title-input::placeholder{color:var(--rv-faint);opacity:.6}
.sv .sv-controls{max-width:860px;margin:0 auto 30px;scroll-margin-top:20px}.sv .sv-search{display:flex;align-items:center;gap:12px;padding:9px 10px 9px 17px;border:1px solid var(--rv-line);border-radius:12px;background:var(--rv-panel);box-shadow:0 3px 12px #00000005;color:var(--rv-faint)}
.sv .sv-search:focus-within{border-color:var(--rv-signal);box-shadow:0 0 0 2px color-mix(in srgb,var(--rv-signal) 12%,transparent)}.sv .sv-search>svg{flex:none}.sv .sv-search input{flex:1;width:100%;border:0;padding:7px 0;background:transparent;font-size:15px;outline:none}.sv .sv-search-scope{display:flex;gap:2px;background:var(--rv-soft);padding:3px;border-radius:7px;flex:none}.sv .sv-search-scope button{border-color:transparent;background:transparent;font-size:11px;padding:6px 9px;color:var(--rv-faint);white-space:nowrap}.sv .sv-search-scope button[aria-pressed=true]{background:var(--rv-panel);color:var(--rv-ink);box-shadow:0 1px 3px #0001}
.sv .sv-toolbar{display:flex;align-items:center;justify-content:space-between;gap:10px;flex-wrap:wrap;padding:10px 0 0}.sv .sv-nav{position:relative;min-width:0}
.sv .sv-pager,.sv .sv-layout{display:inline-flex;align-items:center;gap:2px;padding:3px;border-radius:10px}.sv .sv-pager{border:1px solid var(--rv-line);background:var(--rv-panel);box-shadow:0 1px 2px #0000000a}.sv .sv-layout{border:1px solid transparent;background:var(--rv-soft)}
.sv .sv-pager button,.sv .sv-layout button{height:28px;border:0;border-radius:7px;background:transparent;font-size:12px;white-space:nowrap}.sv .sv-pager button:focus-visible,.sv .sv-layout button:focus-visible{outline-offset:1px}
.sv .sv-pager .sv-step{width:28px;padding:0;color:var(--rv-faint)}.sv .sv-pager .sv-step:hover:not(:disabled){color:var(--rv-ink)}.sv .sv-pager .sv-step:disabled{opacity:.35}
.sv .sv-position{gap:0!important;padding:0 8px 0 11px!important;color:var(--rv-ink);font-variant-numeric:tabular-nums}.sv .sv-position strong{font-weight:600}.sv .sv-position .sv-of{margin-left:4px;color:var(--rv-faint)}.sv .sv-position .sv-agent{max-width:9em;overflow:hidden;text-overflow:ellipsis;margin-right:6px;color:var(--rv-signal)}.sv .sv-caret{margin-left:6px;color:var(--rv-faint);transition:transform .15s}.sv .sv-position[aria-expanded=true]{background:var(--rv-soft)}.sv .sv-position[aria-expanded=true] .sv-caret{transform:rotate(180deg)}
.sv .sv-toolbar-end{display:flex;align-items:center;gap:8px;margin-left:auto}.sv .sv-toolbar .sv-raw-button{height:36px;padding:0 12px;border-radius:10px;color:var(--rv-faint);box-shadow:0 1px 2px #0000000a}.sv .sv-toolbar .sv-raw-button:hover:not(:disabled){background:var(--rv-panel);color:var(--rv-ink)}
.sv .sv-dialog.sv-raw-dialog{width:min(1100px,calc(100% - 32px));max-width:1100px;overflow:hidden}.sv .sv-raw-dialog[open]{display:flex;flex-direction:column}.sv .sv-raw-dialog .sv-dialog-head h2 .sv-meta{margin-left:10px;vertical-align:middle}
.sv .sv-raw-dialog[open]{height:85vh}.sv .sv-raw{position:relative;flex:1;min-height:0;overflow:auto;border-radius:8px;background:var(--rv-soft)}.sv .sv-raw-lines{position:relative;font:12px/19px ui-monospace,monospace}.sv .sv-dialog .sv-raw pre{position:absolute;left:16px;margin:0;padding:0;background:transparent;white-space:pre;overflow-wrap:normal;font:12px/19px ui-monospace,monospace;tab-size:2}.sv .sv-raw-loading{flex:1;display:grid;place-items:center;margin:0}
.sv .sv-layout button{padding:0 12px;color:var(--rv-faint)}.sv .sv-layout button:hover:not(:disabled){background:transparent;color:var(--rv-ink)}.sv .sv-layout button[aria-pressed=true],.sv .sv-layout button[aria-pressed=true]:hover{background:var(--rv-panel);color:var(--rv-ink);box-shadow:0 1px 3px #0000001f}
.sv .sv-reading{max-width:780px;margin:0 auto;min-width:0}.sv .sv-prompt{padding:18px 22px;border-left:3px solid var(--rv-line);background:var(--rv-soft);border-radius:0 8px 8px 0;margin-bottom:32px;overflow-wrap:anywhere}
.sv .sv-label{display:flex;align-items:center;justify-content:space-between;gap:12px;margin:0 0 12px;font:11px ui-monospace,monospace;color:var(--rv-faint);letter-spacing:.08em;text-transform:uppercase}
.sv .sv-label-main{display:inline-flex;align-items:baseline;min-width:0}.sv .sv-time{margin-left:10px;padding-left:10px;border-left:1px solid var(--rv-line);font:11px ui-monospace,monospace;letter-spacing:0;text-transform:none;color:var(--rv-faint);font-variant-numeric:tabular-nums;white-space:nowrap}
.sv .sv-response{overflow-wrap:anywhere}.sv .sv-response-body{font-size:15px;line-height:1.75}
.sv .sv-thinking{white-space:pre-wrap;font-size:13px;line-height:1.7;color:var(--rv-faint);margin:12px 0}
.sv .sv-block{position:relative;min-width:0;border-radius:5px;scroll-margin-top:32px}
.sv .sv-support{margin:30px 0;border-top:1px solid var(--rv-line);padding-top:16px}.sv summary{cursor:pointer;color:var(--rv-faint);font-size:12px;line-height:1.6}.sv .sv-step{padding:24px 0;border-bottom:1px solid var(--rv-line);overflow-wrap:anywhere}.sv .sv-linked{outline:2px solid var(--rv-signal);outline-offset:7px;border-radius:4px}
.sv .sv-notice{padding:14px 18px;background:var(--rv-soft);border-radius:7px;font-size:13px;line-height:1.6}.sv .sv-error{color:var(--rv-error);white-space:pre-wrap}
.sv .sv-outline{border:1px solid var(--rv-line);border-radius:10px;background:var(--rv-panel);padding:10px;margin-top:12px}.sv .sv-outline-list{max-height:340px;overflow:auto;display:grid;gap:4px}.sv .sv-outline-list>button{display:block;text-align:left;border:0;width:100%;padding:11px;background:transparent;line-height:1.5}.sv .sv-outline button[aria-current=true]{background:var(--rv-soft)}.sv .sv-outline .sv-preview{display:block;color:var(--rv-ink);font-size:12px;margin-top:5px;overflow-wrap:anywhere}.sv .sv-child{padding-left:25px!important}.sv .sv-results-head{display:flex;align-items:center;justify-content:space-between;gap:10px;padding:0 8px 6px}
.sv .sv-conversation-card{padding:24px;border:1px solid var(--rv-line);background:var(--rv-panel);border-radius:10px;margin:16px 0;scroll-margin-top:20px}.sv .sv-conversation-card .sv-prompt{margin:16px 0 26px}
.sv .sv-details{max-width:780px;margin:56px auto 0;border-top:1px solid var(--rv-line)}.sv .sv-details>summary{display:flex;align-items:center;flex-wrap:wrap;gap:8px 10px;padding:18px 0;list-style:none;color:var(--rv-ink);font-size:13px}.sv .sv-details>summary::-webkit-details-marker{display:none}
.sv .sv-details-chevron{flex:none;color:var(--rv-faint);transition:transform .15s}.sv .sv-details[open]>summary .sv-details-chevron{transform:rotate(90deg)}.sv .sv-details-title{font-weight:550}.sv .sv-details-synopsis{margin-left:auto;font:12px ui-monospace,monospace;color:var(--rv-faint);font-variant-numeric:tabular-nums}
.sv .sv-facts{display:grid;grid-template-columns:repeat(auto-fill,minmax(200px,1fr));gap:22px 28px;margin:0;padding:22px 24px;border:1px solid var(--rv-line);border-radius:12px;background:var(--rv-panel)}.sv .sv-fact{min-width:0}.sv .sv-fact-wide{grid-column:1/-1}
.sv .sv-fact dt{margin-bottom:6px;font:11px ui-monospace,monospace;letter-spacing:.07em;text-transform:uppercase;color:var(--rv-faint)}.sv .sv-fact dd{margin:0;font-size:14px;line-height:1.45;color:var(--rv-ink);font-variant-numeric:tabular-nums;overflow-wrap:anywhere}.sv .sv-fact-mono dd{font:13px/1.55 ui-monospace,monospace}.sv .sv-fact-error dd{color:var(--rv-error)}.sv .sv-fact-note{display:block;margin-top:3px;font:12px/1.45 system-ui,sans-serif;color:var(--rv-faint)}
.sv .sv-details-actions{display:flex;flex-wrap:wrap;gap:8px;margin:14px 0 4px}.sv .sv-details-actions button{height:34px;padding:0 12px;border-radius:9px;color:var(--rv-faint)}.sv .sv-details-actions button:hover:not(:disabled){background:var(--rv-panel);color:var(--rv-ink)}
.sv .sv-dialog.sv-trace-dialog{width:min(1240px,calc(100% - 32px));max-width:1240px;overflow:hidden}.sv .sv-trace-dialog[open]{display:flex;flex-direction:column}.sv .sv-trace{flex:1;min-height:0;overflow:auto}
.sv .sv-dialog{width:min(960px,calc(100% - 32px));max-width:960px;max-height:85vh;overflow:auto;background:var(--rv-panel);color:var(--rv-ink);border:1px solid var(--rv-line);border-radius:14px;padding:22px}.sv .sv-dialog::backdrop{background:#0006;backdrop-filter:blur(3px)}.sv .sv-dialog-head{display:flex;align-items:center;gap:12px;position:sticky;top:-22px;background:var(--rv-panel);padding:6px 0 12px;z-index:8}.sv .sv-dialog-head strong,.sv .sv-dialog-head h2{margin-right:auto;overflow-wrap:anywhere}.sv .sv-dialog-head h2{font:500 25px "Iowan Old Style",Palatino,Georgia,serif;margin-top:0;margin-bottom:0}.sv .sv-dialog pre{white-space:pre-wrap;overflow-wrap:anywhere;font-size:12px}
.sv .sv-share-dialog{max-width:680px;padding:26px}.sv .sv-share-dialog .sv-dialog-head{top:-26px}.sv .sv-dialog-intro{font-size:13px;line-height:1.65;color:var(--rv-faint);margin:0 0 20px}.sv .sv-share-dialog fieldset{border:0;padding:0;margin:0;min-width:0}
.sv .sv-author-fields{padding:14px 0;border-top:1px solid var(--rv-line);border-bottom:1px solid var(--rv-line);margin-bottom:16px}.sv .sv-author-fields label{display:grid;gap:8px;margin-top:16px;font-size:12px}.sv .sv-author-fields textarea{min-height:110px;resize:vertical;font-size:13px;line-height:1.6}.sv .sv-author-fields input{font-size:14px}
.sv .sv-included-heading{display:flex;align-items:center;justify-content:space-between;gap:10px;margin:10px 0}.sv .sv-included-item{border:1px solid var(--rv-line);border-radius:9px;margin:8px 0}.sv .sv-included-item>summary{padding:12px 14px;list-style:none}.sv .sv-included-item>summary::-webkit-details-marker{display:none}.sv .sv-included-item>summary>span{display:block}.sv .sv-included-item>summary>span:last-child{color:var(--rv-ink);font-size:13px;margin-top:5px}.sv .sv-included-item>summary:after{content:'Expand';font-size:10px;display:block;margin-top:5px}.sv .sv-included-item[open]>summary:after{content:'Collapse'}.sv .sv-included-body{border-top:1px solid var(--rv-line);padding:16px}
.sv .sv-item-actions{display:flex;align-items:center;gap:8px;flex-wrap:wrap;margin-top:14px}.sv .sv-passage{width:100%;min-height:180px;max-height:400px;font:12px/1.6 ui-monospace,monospace;resize:vertical}.sv .sv-reference-options{margin:16px 0}.sv .sv-reference-options label{display:flex;gap:10px;align-items:flex-start;font-size:12px;margin-top:10px;overflow-wrap:anywhere}.sv .sv-share-footer{border-top:1px solid var(--rv-line);padding-top:12px;margin-top:20px}.sv .sv-share-footer .sv-item-actions{justify-content:flex-end}
.sv .sv-saved-notice{display:flex;gap:12px;align-items:center;flex-wrap:wrap;background:var(--rv-soft);padding:14px;border-radius:8px;margin-top:16px;font-size:12px}.sv .sv-saved-notice a,.sv .sv-saved-views a{color:var(--rv-signal)}.sv .sv-saved-views{margin-top:20px}.sv .sv-saved-views a{display:block;padding:8px 0;font-size:13px;overflow-wrap:anywhere}
.sv .sv-outline.sv-nav-outline{position:absolute;top:calc(100% + 8px);left:0;z-index:7;width:min(600px,calc(100vw - 32px));margin:0;padding:6px;border-radius:12px;box-shadow:0 14px 36px #00000024,0 2px 6px #0000000f}.sv .sv-nav-outline .sv-outline-list{max-height:min(420px,60vh);gap:2px}
.sv .sv-nav-outline .sv-outline-list>button{display:grid;grid-template-columns:2.2em minmax(0,1fr);gap:10px;align-items:baseline;padding:9px 12px;border-radius:8px}.sv .sv-outline-n{font:11px ui-monospace,monospace;color:var(--rv-faint);text-align:right;font-variant-numeric:tabular-nums}.sv .sv-outline-text{min-width:0}
.sv .sv-outline-prompt{display:block;overflow:hidden;white-space:nowrap;text-overflow:ellipsis;color:var(--rv-ink);font-size:13px;line-height:1.45}.sv .sv-outline.sv-nav-outline .sv-preview{margin-top:2px;overflow:hidden;white-space:nowrap;text-overflow:ellipsis;color:var(--rv-faint)}
.sv .sv-outline.sv-nav-outline button[aria-current=true]{background:var(--rv-soft);box-shadow:inset 3px 0 0 var(--rv-signal)}.sv .sv-nav-outline button[aria-current=true] .sv-outline-n{color:var(--rv-signal);font-weight:600}
.sv .sv-block button{user-select:none}
.sv .sv-context-menu{position:fixed;z-index:20;display:grid;gap:2px;min-width:210px;max-width:260px;padding:5px;border:1px solid var(--rv-line);border-radius:10px;background:var(--rv-panel);box-shadow:0 12px 32px #00000024,0 2px 6px #0000000f}.sv .sv-context-menu button{justify-content:flex-start;gap:10px;border:0;border-radius:7px;background:transparent;padding:8px 10px;font-size:13px;color:var(--rv-ink)}.sv .sv-context-menu button:disabled{opacity:.45}.sv .sv-context-menu button:hover:not(:disabled),.sv .sv-context-menu button:focus-visible{background:var(--rv-soft);outline:none}.sv .sv-context-menu svg{color:var(--rv-faint)}.sv .sv-context-menu .sv-meta{margin:2px 10px 6px}
@media(max-width:600px){.sv .sv-header{gap:8px;flex-wrap:wrap;margin-bottom:22px}.sv .sv-heading{flex-basis:100%;order:3}.sv h1,.sv .sv-title-input{font-size:25px}.sv .sv-edit-title{opacity:1}.sv .sv-share{font-size:12px;padding:10px 12px}.sv .sv-search{flex-wrap:wrap;gap:8px;padding:10px 12px}.sv .sv-search input{width:calc(100% - 32px);flex:auto}.sv .sv-search-scope{margin-left:26px}.sv .sv-toolbar{gap:8px}.sv .sv-layout button{padding:0 9px}.sv .sv-facts{grid-template-columns:repeat(2,minmax(0,1fr));gap:18px 16px;padding:16px}.sv .sv-details-synopsis{flex-basis:100%;margin-left:25px}.sv .sv-raw-label{display:none}.sv .sv-toolbar .sv-raw-button{width:36px;padding:0}.sv .sv-position{padding:0 6px 0 9px!important}.sv .sv-prompt{padding:16px 14px}.sv .sv-conversation-card{padding:16px}.sv .sv-dialog{padding:18px;max-height:90vh}.sv .sv-dialog-head,.sv .sv-share-dialog .sv-dialog-head{top:-18px}.sv .sv-dialog-head h2{font-size:22px}.sv .sv-included-heading{flex-wrap:wrap}.sv .sv-label{font-size:10px}}
`;

function Inspector({ span, renderSpan, close }: { span: Span; renderSpan: Props["renderSpan"]; close: () => void }) {
  const ref = useRef<HTMLDialogElement>(null), [raw, setRaw] = useState(false);
  useEffect(() => { ref.current?.showModal(); }, []);
  return <dialog className="sv-dialog" ref={ref} onCancel={e => { e.preventDefault(); close(); }} aria-label="Inspect recorded step">
    <div className="sv-dialog-head"><strong>{span.name ?? span.type}</strong><button onClick={() => setRaw(v => !v)}>{raw ? "Formatted" : "Raw data"}</button><button onClick={close}>Close inspection</button></div>
    {raw ? <pre>{JSON.stringify(span, null, 2)}</pre> : renderSpan(span)}
  </dialog>;
}

// A recorded time beside a label.
function Stamp({ at, start, seconds = false }: { at?: string; start?: string; seconds?: boolean }) {
  const time = eventTime(at, start, seconds);
  return time ? <time className="sv-time" dateTime={time.iso} title={time.title} suppressHydrationWarning>{time.label}</time> : null;
}

// The trace explorer, opened from session details rather than unfolding inline.
function TraceDialog({ render, close }: { render: () => ReactNode; close: () => void }) {
  const ref = useRef<HTMLDialogElement>(null);
  useEffect(() => { ref.current?.showModal(); }, []);
  const done = () => { ref.current?.close(); close(); };
  return <dialog className="sv-dialog sv-trace-dialog" ref={ref} onCancel={event => { event.preventDefault(); done(); }} aria-label="Trace explorer">
    <div className="sv-dialog-head"><h2>Trace explorer</h2><button onClick={done}>Close</button></div>
    <div className="sv-trace">{render()}</div>
  </dialog>;
}

// The whole session document as JSON, opened from the full session.
// Pretty-printing a large session is slow, so it happens once per document,
// after the dialog paints, and only the lines on screen are rendered.
const rawDocuments = new WeakMap<Run, { text: string; lines: string[]; bytes: number; widest: number }>();
const RAW_LINE = 19;
function SessionData({ run, close }: { run: Run; close: () => void }) {
  const ref = useRef<HTMLDialogElement>(null), viewport = useRef<HTMLDivElement>(null);
  const [raw, setRaw] = useState(() => rawDocuments.get(run) ?? null), [copied, setCopied] = useState("");
  const [view, setView] = useState({ top: 0, height: 800 });
  useEffect(() => { ref.current?.showModal(); }, []);
  useEffect(() => {
    if (raw) return;
    const handle = window.setTimeout(() => {
      const text = JSON.stringify(run, null, 2), lines = text.split("\n");
      const next = { text, lines, bytes: new Blob([text]).size, widest: lines.reduce((widest, line) => Math.max(widest, line.length), 0) };
      rawDocuments.set(run, next); setRaw(next);
    }, 20);
    return () => window.clearTimeout(handle);
  }, [run, raw]);
  useLayoutEffect(() => {
    const element = viewport.current;
    if (!element) return;
    const measure = () => setView({ top: element.scrollTop, height: element.clientHeight });
    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(element);
    return () => observer.disconnect();
  }, [raw]);
  const size = !raw ? "" : raw.bytes >= 1048576 ? `${(raw.bytes / 1048576).toFixed(1)} MB` : `${Math.max(1, Math.round(raw.bytes / 1024))} KB`;
  const first = Math.max(0, Math.floor(view.top / RAW_LINE) - 30), last = raw ? Math.min(raw.lines.length, Math.ceil((view.top + view.height) / RAW_LINE) + 30) : 0;
  const copy = async () => { if (!raw) return; try { await navigator.clipboard.writeText(raw.text); setCopied("Copied"); } catch { setCopied("Could not copy"); } };
  // Close the modal first; the page behind it can't take focus while it's open.
  const done = () => { ref.current?.close(); close(); };
  return <dialog className="sv-dialog sv-raw-dialog" ref={ref} onCancel={event => { event.preventDefault(); done(); }} aria-label="Session data">
    <div className="sv-dialog-head"><h2>Session data<span className="sv-meta">{run.spans.length} recorded spans{size ? ` · ${size}` : ""}</span></h2><button onClick={copy} disabled={!raw}>{copied || "Copy JSON"}</button><button onClick={done}>Close</button></div>
    {raw ? <div className="sv-raw" ref={viewport} tabIndex={0} role="region" aria-label="Session JSON" onScroll={event => setView({ top: event.currentTarget.scrollTop, height: event.currentTarget.clientHeight })}>
      <div className="sv-raw-lines" style={{ height: raw.lines.length * RAW_LINE + 28, width: `calc(${raw.widest}ch + 32px)` }}>
        <pre style={{ top: 14 + first * RAW_LINE }}>{raw.lines.slice(first, last).join("\n")}</pre>
      </div>
    </div> : <p className="sv-meta sv-raw-loading" role="status">Preparing JSON…</p>}
  </dialog>;
}

export function SessionView({ run, local, renderPart, renderSpan, renderTree, stats }: Props) {
  const exchanges = useMemo(() => exchangesFor(run, readerFlow(buildFlow(run))), [run]);
  // Source text for each selectable part, keyed by its share catalog unit.
  const unitText = useMemo(() => {
    const texts = new Map<string, string>();
    for (const block of exchanges.flatMap(exchange => [...exchange.prompts, ...exchange.blocks]))
      block.msg.content.forEach((part, index) => { if (part.type === "text" || part.type === "thinking") texts.set(partUnit(block, index), part.text); });
    return texts;
  }, [exchanges]);
  const initial = defaultExchange(exchanges);
  const [active, setActive] = useState(initial?.id ?? ""), [mode, setMode] = useState<"exchange" | "conversation">("exchange");
  const [outline, setOutline] = useState(false), [query, setQuery] = useState(""), [limit, setLimit] = useState(50);
  const [searchScope, setSearchScope] = useState<"view" | "session">("view"), [searchOpen, setSearchOpen] = useState(false);
  const [searchTarget, setSearchTarget] = useState<{ key: string; query: string } | null>(null);
  const [panel, setPanel] = useState<"share" | "annotate" | null>(null), [viewDraft, setViewDraft] = useState<ViewDraft | null>(null);
  const preparedScope = useRef(""), preparations = useRef(new Map<string, ViewDraft>());
  const search = useRef<HTMLInputElement>(null), shareButton = useRef<HTMLButtonElement>(null);
  const navRef = useRef<HTMLDivElement>(null), positionButton = useRef<HTMLButtonElement>(null);
  const [menu, setMenu] = useState<{ x: number; y: number; text: string; passage: Passage | null; keyboard: boolean } | null>(null), [passage, setPassage] = useState<Passage | null>(null);
  const menuRef = useRef<HTMLDivElement>(null);
  const [linked, setLinked] = useState<string | null>(null), [inspect, setInspect] = useState<string | null>(null);
  const [title, setTitle] = useState(local?.title || sessionTitle(run, exchanges)), [editing, setEditing] = useState(false), [titleValue, setTitleValue] = useState(title), [titleStatus, setTitleStatus] = useState(""), [titleError, setTitleError] = useState(false);
  const titleSaving = useRef(false), label = sessionLabel(run);
  const [detailsOpen, setDetailsOpen] = useState(false), [traceOpen, setTraceOpen] = useState(false), [rawOpen, setRawOpen] = useState(false);
  const reading = useRef<HTMLDivElement>(null), focusButton = useRef<HTMLElement | null>(null);
  const jumpTo = useRef<string | null>(null), spyHold = useRef(false);
  const restored = useRef(false), state = useRef({ active, mode }); state.current = { active, mode };
  const storeKey = `slink:reading:${local?.source ?? readingKey(run, exchanges)}`;
  const current = exchanges.find(e => e.id === active) ?? initial;
  const main = exchanges.filter(e => !e.child), position = current ? (current.child ? exchanges : main).indexOf(current) : -1;
  const sequence = current?.child ? exchanges : main;

  useEffect(() => { if (local) document.title = `${title || label} · session.link`; }, [title, label, local]);

  useLayoutEffect(() => {
    const arrive = () => {
      const params = new URLSearchParams(location.hash.slice(1)), message = params.get("message"), span = params.get("span"), exchangeID = params.get("exchange");
      // Older links can name a message that no longer starts its own exchange.
      const found = exchanges.find(e => e.id === exchangeID || [...e.prompts, ...e.blocks].some(b => message ? b.key === message : span ? b.spanId === span : b.key === exchangeID));
      if (found) {
        const blocks = [...found.prompts, ...found.blocks];
        const target = blocks.find(b => b.key === message) ?? blocks.filter(b => b.spanId === span && b.msg.role === "assistant" && messageText(b.msg)).at(-1) ?? blocks.find(b => b.spanId === span);
        setActive(found.id); setMode("exchange"); setLinked(target?.key ?? null); setSearchTarget(null); return true;
      }
      if (span && run.spans.some(s => s.id === span)) { setInspect(span); setSearchTarget(null); return true; }
      return false;
    };
    let frame = 0;
    if (!arrive()) {
      try {
        const saved = JSON.parse(localStorage.getItem(storeKey) ?? "null");
        if (saved && exchanges.some(e => e.id === saved.active)) {
          setActive(saved.active); setMode(saved.mode === "conversation" ? "conversation" : "exchange");
          frame = requestAnimationFrame(() => {
            const anchor = saved.mode === "conversation" ? document.getElementById(`exchange-${saved.active}`) : null;
            window.scrollTo(0, anchor ? window.scrollY + anchor.getBoundingClientRect().top - (Number(saved.offset) || 0) : Math.max(0, Number(saved.y) || 0));
          });
        }
      } catch { /* Reading works with storage disabled. */ }
    }
    restored.current = true;
    const changed = () => { arrive(); };
    window.addEventListener("hashchange", changed);
    return () => { cancelAnimationFrame(frame); window.removeEventListener("hashchange", changed); };
  }, [run, exchanges, storeKey]);

  useEffect(() => {
    const save = () => { if (restored.current) {
      const anchor = state.current.mode === "conversation" ? Array.from(document.querySelectorAll<HTMLElement>(".sv-conversation-card")).find(el => el.getBoundingClientRect().bottom > 0) : null;
      try { localStorage.setItem(storeKey, JSON.stringify({ ...state.current, ...(anchor ? { active: anchor.id.slice("exchange-".length), offset: anchor.getBoundingClientRect().top } : {}), y: window.scrollY })); } catch { /* Optional storage. */ }
    } };
    let frame = 0;
    const scroll = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(save); };
    save(); window.addEventListener("scroll", scroll, { passive: true }); window.addEventListener("pagehide", save);
    return () => { cancelAnimationFrame(frame); window.removeEventListener("scroll", scroll); window.removeEventListener("pagehide", save); };
  }, [storeKey, active, mode]);

  // Entering the full session keeps your place. After that, scrolling moves
  // the position; the position never pulls the page back.
  useLayoutEffect(() => {
    if (mode === "conversation") {
      if (!jumpTo.current) return;
      spyHold.current = true;
      document.getElementById(`exchange-${jumpTo.current}`)?.scrollIntoView({ block: "start" });
      jumpTo.current = null;
    } else if (linked && !searchTarget) document.getElementById(`message-${linked}`)?.scrollIntoView({ block: "center" });
  }, [mode, linked, active, searchTarget]);

  // In the full session, the position follows the card at the top of the screen.
  useEffect(() => {
    if (mode !== "conversation") return;
    let frame = 0;
    const spy = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(() => {
      if (spyHold.current) return;
      const cards = Array.from(document.querySelectorAll<HTMLElement>(".sv-conversation-card"));
      const atBottom = window.innerHeight + window.scrollY >= document.documentElement.scrollHeight - 2;
      const card = atBottom ? cards.at(-1) : cards.find(el => el.getBoundingClientRect().bottom > 96) ?? cards.at(-1);
      const id = card?.id.slice("exchange-".length);
      if (id) setActive(previous => previous === id ? previous : id);
    }); };
    // A jump holds the position until the reader scrolls by hand.
    const release = () => { spyHold.current = false; };
    const inputs = ["wheel", "touchstart", "keydown", "pointerdown"] as const;
    window.addEventListener("scroll", spy, { passive: true }); inputs.forEach(type => window.addEventListener(type, release, { passive: true }));
    return () => { cancelAnimationFrame(frame); window.removeEventListener("scroll", spy); inputs.forEach(type => window.removeEventListener(type, release)); };
  }, [mode]);

  useLayoutEffect(() => {
    if (!searchTarget) return;
      const root = document.getElementById(`message-${searchTarget.key}`);
      if (!root) return;
      const walker = document.createTreeWalker(root, NodeFilter.SHOW_TEXT);
      let node: Node | null;
      while ((node = walker.nextNode())) {
        if (node.parentElement?.closest("button")) continue;
        const at = (node.textContent ?? "").toLowerCase().indexOf(searchTarget.query);
        if (at < 0) continue;
        const range = document.createRange();
        range.setStart(node, at); range.setEnd(node, Math.min((node.textContent ?? "").length, at + searchTarget.query.length));
        if (!range.getClientRects().length) continue;
        window.scrollTo({ top: Math.max(0, window.scrollY + range.getBoundingClientRect().top - 160) });
        break;
      }
  }, [searchTarget, active, linked]);

  // The selection menu closes on a click elsewhere, Escape, scrolling or resizing.
  useEffect(() => {
    if (!menu) return;
    // Keyboard users land on the first item; a mouse keeps the selection highlighted.
    if (menu.keyboard) menuRef.current?.querySelector<HTMLButtonElement>("button:not(:disabled)")?.focus({ preventScroll: true });
    const close = () => setMenu(null);
    const away = (event: MouseEvent) => { if (!menuRef.current?.contains(event.target as Node)) close(); };
    const key = (event: KeyboardEvent) => {
      if (event.key === "Escape") close();
      if (event.key !== "ArrowDown" && event.key !== "ArrowUp") return;
      event.preventDefault();
      const items = Array.from(menuRef.current?.querySelectorAll<HTMLButtonElement>("button:not(:disabled)") ?? []);
      const at = items.indexOf(document.activeElement as HTMLButtonElement);
      items[(at + (event.key === "ArrowDown" ? 1 : items.length - 1) + items.length) % items.length]?.focus();
    };
    document.addEventListener("mousedown", away); document.addEventListener("keydown", key);
    window.addEventListener("scroll", close, { passive: true }); window.addEventListener("resize", close); window.addEventListener("blur", close);
    return () => { document.removeEventListener("mousedown", away); document.removeEventListener("keydown", key); window.removeEventListener("scroll", close); window.removeEventListener("resize", close); window.removeEventListener("blur", close); };
  }, [menu]);

  // The outline is a popover: a click elsewhere or Escape closes it.
  useEffect(() => {
    if (!outline) return;
    const away = (event: MouseEvent) => { if (!navRef.current?.contains(event.target as Node)) setOutline(false); };
    const key = (event: KeyboardEvent) => { if (event.key === "Escape") { setOutline(false); positionButton.current?.focus(); } };
    document.addEventListener("mousedown", away); document.addEventListener("keydown", key);
    document.querySelector<HTMLElement>("#sv-outline [aria-current=true]")?.scrollIntoView({ block: "nearest" });
    return () => { document.removeEventListener("mousedown", away); document.removeEventListener("keydown", key); };
  }, [outline]);

  const navigate = (e: Exchange, scroll = true) => {
    setActive(e.id); setMode("exchange"); setLinked(null); setSearchTarget(null); setOutline(false);
    // Browsing doesn't create a deep link.
    if (location.hash) history.replaceState(null, "", withAnchor(location.href, ""));
    if (scroll) reading.current?.scrollIntoView({ block: "start" });
  };
  const startTitle = () => { setTitleValue(title); setTitleStatus(""); setTitleError(false); setEditing(true); };
  const cancelTitle = () => { titleSaving.current = false; setEditing(false); setTitleValue(title); };
  // Move within the current layout: the full session scrolls to the card
  // instead of switching back to one exchange at a time.
  const moveTo = (e: Exchange) => {
    if (mode !== "conversation") { navigate(e); return; }
    setOutline(false); setActive(e.id);
    spyHold.current = true;
    document.getElementById(`exchange-${e.id}`)?.scrollIntoView({ block: "start", behavior: matchMedia("(prefers-reduced-motion: reduce)").matches ? "auto" : "smooth" });
  };
  const saveTitle = async () => {
    // Enter and blur both land here; the first one wins.
    if (!local || titleSaving.current) return;
    const next = titleValue.trim();
    if (!next || next === title) { cancelTitle(); return; }
    titleSaving.current = true; setEditing(false); setTitle(next); setTitleError(false); setTitleStatus("Saving…");
    try {
      const response = await fetch(`/api/title/${local.source}`, { method: "PUT", headers: { "x-slink": "1", "content-type": "application/json" }, body: JSON.stringify({ title: next }) });
      if (!response.ok) throw new Error("Could not save the title.");
      setTitleStatus("Saved");
      window.setTimeout(() => setTitleStatus(current => current === "Saved" ? "" : current), 2400);
    } catch (error) { setTitle(title); setTitleValue(next); setTitleError(true); setTitleStatus(error instanceof Error ? error.message : String(error)); }
    finally { titleSaving.current = false; }
  };
  const focusedResponse = current?.blocks.find(block => block.key === linked && block.msg.role === "assistant" && messageText(block.msg)) ?? (current && responseFor(current));
  const visibleExchanges = mode === "conversation" ? exchanges : current ? [current] : [];
  const defaults = visibleExchanges.flatMap(exchange => {
    const response = exchange === current ? focusedResponse : responseFor(exchange);
    const failure = exchange.blocks.at(-1);
    return [...exchange.prompts, ...(response ? [response] : []), ...(failure?.err ? [failure] : [])];
  });
  const prefixes = defaults.flatMap(selectionPrefixes);
  const activity = visibleExchanges.flatMap(exchange => exchange.blocks).filter(block => !defaults.includes(block)).flatMap(selectionPrefixes);
  // What the capture contains, summarized at the end of the page.
  const facts = useMemo((): Fact[] => {
    const meta = (run.metadata ?? {}) as Record<string, unknown>;
    const text = (value: unknown) => typeof value === "string" && value.trim() ? value : undefined;
    const exact = (n: number) => n.toLocaleString();
    const compact = (n: number) => n.toLocaleString(undefined, { notation: "compact", maximumFractionDigits: 1 });
    const plural = (n: number, one: string, many: string) => `${exact(n)} ${n === 1 ? one : many}`;
    const project = local?.project ?? text(meta.cwd), version = text(meta.harness_version), sessionId = text(meta.session_id);
    const fidelity = ({ exact: "Exact capture", reconstructed: "Reconstructed", partial: "Partial capture" } as Record<string, string>)[String(run.source?.fidelity ?? "")];
    const started = new Date(run.created_at), valid = !Number.isNaN(started.getTime());
    const mainCount = exchanges.filter(exchange => !exchange.child).length, subagents = exchanges.length - mainCount;
    const list: (Fact | false)[] = [
      { label: "Source", value: harnessName(run), note: [version && `Version ${version}`, fidelity].filter(Boolean).join(" · ") || undefined },
      !!project && { label: "Project", value: project.split(/[\\/]/).filter(Boolean).at(-1) ?? project, title: project },
      { label: "Started", value: valid ? started.toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }) : run.created_at, title: valid ? started.toLocaleString(undefined, { dateStyle: "full", timeStyle: "long" }) : undefined },
      !!duration(stats.durMs ?? 0) && { label: "Duration", value: duration(stats.durMs ?? 0) },
      { label: "Human input", value: plural(mainCount, "message", "messages"), note: subagents ? `${exact(subagents)} more in subagents` : undefined },
      { label: "Recorded spans", value: exact(run.spans.length), note: `${plural(stats.modelCalls, "model call", "model calls")} · ${plural(stats.toolCalls, "tool call", "tool calls")}` },
      (stats.tokensIn != null || stats.tokensOut != null) && { label: "Tokens", value: `${compact(stats.tokensIn ?? 0)} in · ${compact(stats.tokensOut ?? 0)} out`, title: `${exact(stats.tokensIn ?? 0)} in · ${exact(stats.tokensOut ?? 0)} out` },
      stats.cost != null && { label: "Cost", value: `$${stats.cost < 0.01 ? stats.cost.toFixed(4) : stats.cost.toFixed(2)}` },
      { label: "Errors", value: stats.errors ? exact(stats.errors) : "None", error: stats.errors > 0 },
      stats.models.length > 0 && { label: stats.models.length === 1 ? "Model" : "Models", value: stats.models.join(" · "), wide: true, mono: true },
      !!sessionId && { label: "Session ID", value: sessionId, wide: true, mono: true },
    ];
    return list.filter((fact): fact is Fact => !!fact);
  }, [run, stats, exchanges, local]);
  const synopsis = [`${main.length} ${main.length === 1 ? "message" : "messages"}`, duration(stats.durMs ?? 0), stats.errors ? `${stats.errors} ${stats.errors === 1 ? "error" : "errors"}` : ""].filter(Boolean).join(" · ");
  const shareTitle = title || shortText(main.map(promptText).find(Boolean) ?? label, 90);
  // Maps selected text inside one rendered part to its source passage.
  const passageFor = (range: Range, text: string): Passage | null => {
    const elementOf = (node: Node) => node instanceof Element ? node : node.parentElement;
    const part = elementOf(range.startContainer)?.closest<HTMLElement>("[data-unit]");
    const unit = part?.dataset.unit, source = unit ? unitText.get(unit) : undefined;
    if (!part || !unit || source == null || elementOf(range.endContainer)?.closest("[data-unit]") !== part) return null;
    const before = document.createRange();
    before.selectNodeContents(part); before.setEnd(range.startContainer, range.startOffset);
    const found = sourceRange(source, text, before.toString().length / (part.textContent?.length || 1));
    return found ? { unit, ...found } : null;
  };
  // Right-clicking selected text offers to comment on and share that passage.
  const openMenu = (event: ReactMouseEvent) => {
    const selection = window.getSelection();
    if (!local || !selection || selection.isCollapsed || !selection.rangeCount) return;
    const range = selection.getRangeAt(0), text = selection.toString();
    const keyboard = event.clientX === 0 && event.clientY === 0;
    const onSelection = keyboard || Array.from(range.getClientRects()).some(r => event.clientX >= r.left - 2 && event.clientX <= r.right + 2 && event.clientY >= r.top - 2 && event.clientY <= r.bottom + 2);
    if (!text.trim() || !onSelection) return;
    event.preventDefault();
    const box = range.getBoundingClientRect();
    setMenu({ x: keyboard ? box.left : event.clientX, y: keyboard ? box.bottom : event.clientY, text, passage: passageFor(range, text), keyboard });
  };
  const commentOn = (selected: Passage) => {
    const scope = `passage:${selected.unit}:${selected.start}-${selected.end}`;
    setViewDraft(preparations.current.get(scope) ?? { title: shareTitle, note: "", items: [{ id: selected.unit, start: selected.start, end: selected.end }], primary: selected.unit });
    preparedScope.current = scope; setPassage(selected); setMenu(null);
    focusButton.current = shareButton.current; setPanel("annotate");
    window.getSelection()?.removeAllRanges();
  };
  const copySelection = async (text: string) => {
    setMenu(null);
    try { await navigator.clipboard.writeText(text); } catch { /* The browser's own copy shortcut still works. */ }
  };
  // Preparations are kept per scope while the reader stays open.
  const openPanel = (button: HTMLButtonElement) => {
    const scope = JSON.stringify(prefixes);
    setViewDraft(preparations.current.get(scope) ?? null);
    preparedScope.current = scope; setPassage(null);
    focusButton.current = button; setPanel("share");
  };
  const closePanel = () => { setPanel(null); focusButton.current?.focus({ preventScroll: true }); };
  const closeInspector = () => { setInspect(null); focusButton.current?.focus({ preventScroll: true }); };
  const showPart = (part: ContentPart, full = false) => reasoningUnavailable(part, run) ? null : part.type === "thinking" ? <div className="sv-thinking">{part.text}</div> : renderPart(part, full);
  const showBlock = (block: MessageBlock, full = false, collapsed = false) => <div id={`message-${block.key}`} key={block.key} className={`sv-block${linked === block.key ? " sv-linked" : ""}`}>
    {block.err && <p className="sv-notice sv-error">{block.err}</p>}
    {collapsed ? <details open={linked === block.key || undefined}><summary>{shortText(messageText(block.msg), 180)} · Full prompt</summary>{block.msg.content.map((part, i) => <div key={i} data-unit={partUnit(block, i)}>{showPart(part, true)}</div>)}</details> : block.msg.content.map((part, i) => <div key={i} data-unit={partUnit(block, i)}>{showPart(part, full)}</div>)}
  </div>;

  const exchangeBody = (exchange: Exchange, compact = false) => {
    const selected = exchange.blocks.find(b => b.key === linked && b.msg.role === "assistant" && messageText(b.msg));
    const response = selected ?? responseFor(exchange);
    const supporting = exchange.blocks.filter(b => b !== response);
    const unavailableReasoning = supporting.flatMap(block => block.msg.content).filter(part => reasoningUnavailable(part, run));
    const visibleSupporting = supporting.filter(block => block.err || block.msg.content.some(part => !reasoningUnavailable(part, run)));
    const unavailableTarget = supporting.find(block => block.key === linked && !visibleSupporting.includes(block));
    const encryptedReasoning = unavailableReasoning.some(part => part.type === "thinking" && (part.reason === "encrypted" || part.text === "[reasoning]"));
    const readableReasoning = supporting.some(block => block.msg.content.some(part => part.type === "thinking" && !reasoningUnavailable(part, run)));
    const last = exchange.blocks.at(-1), endedWithError = last?.err && (!response || exchange.blocks.indexOf(last) >= exchange.blocks.indexOf(response));
    return <>
      {exchange.prompts.length > 0 && <div className="sv-prompt"><p className="sv-label"><span className="sv-label-main">Human input<Stamp at={exchange.prompts[0].at} start={run.created_at} /></span></p>{exchange.prompts.map(block => {
        const long = messageText(block.msg).length > 600;
        return showBlock(block, true, long);
      })}</div>}
      {!exchange.prompts.length && <p className="sv-meta">Human input not captured.</p>}
      {endedWithError && <p className="sv-notice sv-error">Error: {last.err}</p>}
      {response ? <section className="sv-response" aria-label="Agent response">
        <div className="sv-label"><span className="sv-label-main">Agent response<Stamp at={response.at} start={run.created_at} /></span></div>
        <div className="sv-response-body">{showBlock(response, !compact)}</div>
      </section> : <div className="sv-notice">No response captured.{run.metadata?.in_progress === true && exchange.id === initial?.id ? " The session was still recording when this snapshot was saved." : ""}</div>}
      {supporting.length > 0 && <SupportingSteps key={`${exchange.id}-${linked}`} count={visibleSupporting.length} duration={elapsed(exchange.prompts[0]?.at, exchange.blocks.map(block => block.at).filter(Boolean).at(-1))} initiallyOpen={supporting.some(b => b.key === linked)} render={() => <>
        {unavailableReasoning.length > 0 && <p className={`sv-notice sv-reasoning-unavailable${unavailableTarget ? " sv-linked" : ""}`} id={unavailableTarget ? `message-${unavailableTarget.key}` : undefined}>{readableReasoning ? "Some reasoning text isn’t available in this capture." : "Reasoning text isn’t available in this capture."}{encryptedReasoning ? " The source contains encrypted reasoning without a readable summary." : " Reasoning events were recorded without readable text."}</p>}
        {visibleSupporting.map(block => <div className="sv-step" key={block.key}><div className="sv-label"><span className="sv-label-main">{block.err ? "Error" : block.msg.content.length > 0 && block.msg.content.every(part => part.type === "thinking") ? "Reasoning" : block.msg.content.some(part => part.type === "tool_call") ? "Tool call" : block.msg.role === "assistant" ? "Agent message" : readingRole(block.msg) === "context" ? "Provided context" : readingRole(block.msg) === "tool" ? "Tool result" : block.msg.role}<Stamp at={block.at} start={run.created_at} seconds /></span></div>{showBlock(block)}</div>)}
      </>} />}

    </>;
  };

  // Rendered messages depend only on what they show. Reading position, search
  // typing and panel edits must not re-render or re-parse them.
  const focusedBody = useMemo(() => mode === "exchange" && current ? exchangeBody(current) : null, [mode, current, linked, run, local, renderPart]);
  const conversation = useMemo(() => mode === "conversation" ? <Conversation exchanges={exchanges} render={exchangeBody} /> : null, [mode, exchanges, linked, run, local, renderPart]);

  const matches = useMemo(() => {
    const q = query.trim().toLowerCase();
    if (!q) return [];
    const scope = searchScope === "session" || mode === "conversation" ? exchanges : current ? [current] : [];
    const hits = scope.flatMap(exchange => [...exchange.prompts, ...exchange.blocks].filter(block => searchableBlock(block).lower.includes(q)).map(block => ({ exchange, block, span: block.spanId, entry: searchableBlock(block) })));
    if (searchScope === "session") {
      const found = new Set(hits.map(hit => hit.span));
      return [...hits, ...run.spans.filter(span => !found.has(span.id) && searchableSpan(span).lower.includes(q)).map(span => ({ exchange: undefined, block: undefined, span: span.id, entry: searchableSpan(span) }))];
    }
    return hits;
  }, [exchanges, current, mode, searchScope, query, run]);
  const snippet = ({ text, lower }: { text: string; lower: string }) => {
    const at = Math.max(0, lower.indexOf(query.trim().toLowerCase()));
    return `${at > 50 ? "…" : ""}${shortText(text.slice(Math.max(0, at - 50), at + 400), 170)}`;
  };
  const inspected = run.spans.find(s => s.id === inspect);
  return <div className="rv sv"><style>{CSS}</style>
    <header className={`sv-header${local ? " sv-local" : ""}`}>
      {local && <a className="sv-back" href="/" aria-label="All sessions" title="All sessions"><ArrowLeft size={17} /></a>}
      <div className="sv-heading">
        {editing ? <input className="sv-title-input" aria-label="Session title" placeholder="Name this session" value={titleValue} maxLength={256} autoFocus onChange={e => setTitleValue(e.target.value)} onKeyDown={e => { if (e.key === "Escape") cancelTitle(); if (e.key === "Enter") { e.preventDefault(); void saveTitle(); } }} onBlur={() => { void saveTitle(); }} />
          : title ? <h1 onClick={local ? startTitle : undefined}>{title}</h1> : <h1 className="sv-untitled" onClick={local ? startTitle : undefined}>{label}</h1>}
        {local && !editing && <button className="sv-quiet sv-edit-title" aria-label="Edit title" title="Edit title" onClick={startTitle}><Pencil size={14} /></button>}
        {titleStatus && <span className={`sv-meta sv-title-status${titleError ? " sv-error" : ""}`} role="status">{titleStatus}</span>}
      </div>
      {local && <button ref={shareButton} className="sv-primary sv-share" disabled={!prefixes.length} onClick={event => openPanel(event.currentTarget)}><Share2 size={15} />Share this view</button>}
    </header>
    <div className="sv-controls" ref={reading}>
      <div className="sv-search" role="search"><Search size={19} aria-hidden="true" /><input ref={search} type="search" aria-label="Search session content" aria-controls="sv-search-results" aria-expanded={!!query.trim() && searchOpen} placeholder={searchScope === "view" ? "Search this view…" : "Search the whole session…"} value={query} onFocus={() => setSearchOpen(true)} onChange={event => { setQuery(event.target.value); setLimit(50); setSearchOpen(true); setOutline(false); }} onKeyDown={event => { if (event.key === "Escape") { setSearchOpen(false); search.current?.blur(); } if (event.key === "ArrowDown") { event.preventDefault(); document.querySelector<HTMLButtonElement>("#sv-search-results .sv-outline-list button")?.focus(); } }} />
        <div className="sv-search-scope" role="group" aria-label="Search scope"><button aria-pressed={searchScope === "view"} onClick={() => { setSearchScope("view"); setSearchOpen(true); setLimit(50); }}>This view</button><button aria-pressed={searchScope === "session"} onClick={() => { setSearchScope("session"); setSearchOpen(true); setLimit(50); }}>Whole session</button></div>
      </div>
      <nav className="sv-toolbar" aria-label="Session navigation">
        {current && <div className="sv-nav" ref={navRef}>
          <div className="sv-pager">
            <button className="sv-step" aria-label="Previous in conversation" title="Previous" disabled={position <= 0} onClick={() => moveTo(sequence[position - 1])}><ChevronLeft size={16} /></button>
            <button ref={positionButton} className="sv-position" aria-expanded={outline} aria-controls="sv-outline" title="Conversation outline" onClick={() => { setOutline(value => !value); setSearchOpen(false); }}>{current.child && <><span className="sv-agent">{current.agent ?? "Agent"}</span>{" "}</>}<strong>{position + 1}</strong>{" "}<span className="sv-of">of {sequence.length}</span><ChevronDown size={14} className="sv-caret" aria-hidden="true" /></button>
            <button className="sv-step" aria-label="Next in conversation" title="Next" disabled={position >= sequence.length - 1} onClick={() => moveTo(sequence[position + 1])}><ChevronRight size={16} /></button>
          </div>
          {outline && <section id="sv-outline" className="sv-outline sv-nav-outline" aria-label="Conversation outline"><div className="sv-outline-list">{exchanges.map(exchange => <button key={exchange.id} className={exchange.child ? "sv-child" : undefined} aria-current={exchange.id === active} onClick={() => moveTo(exchange)}><span className="sv-outline-n">{(exchange.child ? exchanges : main).indexOf(exchange) + 1}</span><span className="sv-outline-text"><span className="sv-outline-prompt">{exchange.child ? `${exchange.agent ?? "Agent"} · ` : ""}{previewText(promptLabel(exchange), 120)}</span><span className="sv-preview">{previewText(messageText(responseFor(exchange)?.msg ?? { role: "assistant", content: [] }), 140) || "No response captured"}</span></span></button>)}</div></section>}
        </div>}
        <div className="sv-toolbar-end">
          {mode === "conversation" && <button className="sv-raw-button" aria-label="Raw data" title="Open the whole session as JSON" onClick={event => { focusButton.current = event.currentTarget; setRawOpen(true); }}><FileJson size={14} aria-hidden="true" /><span className="sv-raw-label">Raw data</span></button>}
          <div className="sv-layout" role="group" aria-label="Reading layout">
            <button aria-pressed={mode === "exchange"} onClick={() => { setMode("exchange"); setOutline(false); }}>Focused</button>
            <button aria-pressed={mode === "conversation"} onClick={() => { if (mode !== "conversation") jumpTo.current = active; setMode("conversation"); setOutline(false); }}>Full session</button>
          </div>
        </div>
      </nav>
      {!!query.trim() && searchOpen && <section id="sv-search-results" className="sv-outline sv-results" aria-label="Search results" onKeyDown={event => { if (event.key === "Escape") { search.current?.focus(); setSearchOpen(false); } }}>
        <div className="sv-results-head"><span className="sv-meta" role="status">{matches.length} {matches.length === 1 ? "match" : "matches"} · {searchScope === "view" ? "this view" : "whole session"}</span><button className="sv-quiet" onClick={() => setSearchOpen(false)}>Close results</button></div>
        <div className="sv-outline-list">{matches.slice(0, limit).map(hit => <button key={hit.block?.key ?? `span-${hit.span}`} onClick={() => { if (hit.exchange) { if (mode === "conversation") { setActive(hit.exchange.id); spyHold.current = true; } else navigate(hit.exchange, false); setLinked(hit.block?.key ?? null); if (hit.block) setSearchTarget({ key: hit.block.key, query: query.trim().toLowerCase() }); } else { focusButton.current = search.current; setInspect(hit.span); } setSearchOpen(false); }}><span className="sv-meta">{hit.block ? hit.block.err ? "Error" : readingRole(hit.block.msg) === "human" ? "Human input" : readingRole(hit.block.msg) === "context" ? "Provided context" : hit.exchange && responseFor(hit.exchange) === hit.block ? "Agent response" : "Agent activity" : "Raw data"}{hit.exchange?.child ? ` · ${hit.exchange.agent ?? "Subagent"}` : ""}</span><span className="sv-preview">{snippet(hit.entry)}</span></button>)}
          {!matches.length && <p className="sv-meta">No matches.{searchScope === "view" && <> <button className="sv-quiet" onClick={() => setSearchScope("session")}>Search the whole session</button></>}</p>}{matches.length > limit && <button onClick={() => setLimit(value => value + 50)}>Show more matches</button>}
        </div>
      </section>}
    </div>
    <div className="sv-reading" onContextMenu={openMenu}>{mode === "exchange" ? focusedBody ?? <p className="sv-notice">No conversation was captured. Open session details to inspect the recorded data.</p> : conversation}</div>
    <details className="sv-details" open={detailsOpen} onToggle={event => setDetailsOpen(event.currentTarget.open)}>
      <summary><ChevronRight size={15} className="sv-details-chevron" aria-hidden="true" /><span className="sv-details-title">Session details</span><span className="sv-details-synopsis">{synopsis}</span></summary>
      {detailsOpen && <>
        <dl className="sv-facts">{facts.map(fact => <div key={fact.label} className={`sv-fact${fact.wide ? " sv-fact-wide" : ""}${fact.mono ? " sv-fact-mono" : ""}${fact.error ? " sv-fact-error" : ""}`}><dt>{fact.label}</dt><dd title={fact.title}>{fact.value}{fact.note && <span className="sv-fact-note">{fact.note}</span>}</dd></div>)}</dl>
        <div className="sv-details-actions">
          <button onClick={event => { focusButton.current = event.currentTarget; setRawOpen(true); }}><FileJson size={14} aria-hidden="true" />Raw data</button>
          <button onClick={event => { focusButton.current = event.currentTarget; setTraceOpen(true); }}><ListTree size={14} aria-hidden="true" />Trace explorer</button>
        </div>
      </>}
    </details>
    {panel && local && <ViewDialog source={local.source} title={shareTitle} prefixes={panel === "annotate" && passage ? [passage.unit] : prefixes} primary={panel === "annotate" && passage ? passage.unit : prefixes[0]} activity={panel === "annotate" ? [] : activity} explicit={panel === "annotate"} reconcile={false} intent={panel} draft={viewDraft} onDraft={draft => { setViewDraft(draft); preparations.current.set(preparedScope.current, draft); }} close={closePanel} />}
    {inspected && <Inspector key={inspected.id} span={inspected} renderSpan={renderSpan} close={closeInspector} />}
    {rawOpen && <SessionData run={run} close={() => { setRawOpen(false); focusButton.current?.focus({ preventScroll: true }); }} />}
    {traceOpen && <TraceDialog render={renderTree} close={() => { setTraceOpen(false); focusButton.current?.focus({ preventScroll: true }); }} />}
    {menu && <div ref={menuRef} className="sv-context-menu" role="menu" aria-label="Selected text" style={{ left: Math.max(8, Math.min(menu.x, window.innerWidth - 268)), top: Math.max(8, Math.min(menu.y, window.innerHeight - 140)) }}>
      <button role="menuitem" disabled={!menu.passage} onClick={() => { if (menu.passage) commentOn(menu.passage); }}><MessageSquare size={15} aria-hidden="true" />Comment and share</button>
      <button role="menuitem" onClick={() => copySelection(menu.text)}><Copy size={15} aria-hidden="true" />Copy</button>
      {!menu.passage && <p className="sv-meta">Select text within one message to share it.</p>}
    </div>}
  </div>;
}

const canonicalMessage = (block: MessageBlock) => [messageText(block.msg), block.err ?? "", ...block.msg.content.filter(p => p.type !== "text").map(p => JSON.stringify(p))].join("\n");
// Search text is derived once per message and span, not on every keystroke.
const blockSearch = new WeakMap<MessageBlock, { text: string; lower: string }>(), spanSearch = new WeakMap<Span, { text: string; lower: string }>();
const searchable = <T extends object>(cache: WeakMap<T, { text: string; lower: string }>, item: T, read: (item: T) => string) => {
  let entry = cache.get(item);
  if (!entry) { const text = read(item); entry = { text, lower: text.toLowerCase() }; cache.set(item, entry); }
  return entry;
};
const searchableBlock = (block: MessageBlock) => searchable(blockSearch, block, canonicalMessage);
const searchableSpan = (span: Span) => searchable(spanSearch, span, item => JSON.stringify(item));

function SupportingSteps({ count, duration, initiallyOpen, render }: { count: number; duration: string; initiallyOpen: boolean; render: () => ReactNode }) {
  const [open, setOpen] = useState(initiallyOpen);
  return <details className="sv-support" open={open} onToggle={event => setOpen(event.currentTarget.open)}><summary>Agent activity{count > 0 ? ` · ${count} ${count === 1 ? "step" : "steps"}` : ""}{duration && <span className="sv-duration" title="Time from the human input to the last recorded step"> · {duration}</span>}</summary>{open && render()}</details>;
}

// The full session renders every exchange; it never paginates.
function Conversation({ exchanges, render }: { exchanges: Exchange[]; render: (e: Exchange, compact: boolean) => ReactNode }) {
  // Positions match the navigation row: main exchanges count among themselves,
  // subagent exchanges by their place in the whole session. The interface
  // shows no noun for the unit until a better name than "exchange" is chosen.
  const main = exchanges.filter(e => !e.child);
  const position = (e: Exchange) => e.child ? `${e.agent ?? "Agent"} · ${exchanges.indexOf(e) + 1} of ${exchanges.length}` : `${main.indexOf(e) + 1} of ${main.length}`;
  return <>{exchanges.map(e => <article id={`exchange-${e.id}`} className="sv-conversation-card" key={e.id}><div className="sv-label"><span>{position(e)}</span></div>{render(e, true)}</article>)}</>;
}
