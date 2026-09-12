import { useEffect, useState, type ReactNode } from "react";
import type { Run } from "@session-link/format";
import { withAnchor } from "./encryption";

export const SHARE_EXTENSION = "session_link.share.v1";
type Item = { id: string; kind: string; role: string; passage?: boolean; omitted_before?: boolean; prompt_missing?: boolean; tool_call_missing?: boolean };
type Share = { kind: "excerpt"; primary_id: string; note_id?: string; original_fidelity?: string; references_unavailable?: boolean; items: Item[] };

export function shareInfo(run: Run): Share | null {
  const value = run.extensions?.[SHARE_EXTENSION] as Partial<Share> | undefined;
  if (value?.kind !== "excerpt" || typeof value.primary_id !== "string" || !Array.isArray(value.items)) return null;
  if (!value.items.every(item => item && typeof item.id === "string" && typeof item.kind === "string" && typeof item.role === "string")) return null;
  return value as Share;
}

const CSS = `
.sh{max-width:820px;margin:24px auto;color:var(--rv-ink);font-family:system-ui,sans-serif}
.sh h1{font:500 32px "Iowan Old Style",Palatino,Georgia,serif;margin:12px 0;overflow-wrap:anywhere}
.sh .sh-meta{font:11px ui-monospace,monospace;letter-spacing:.08em;color:var(--rv-faint);text-transform:uppercase}
.sh .sh-intro{font-size:13px;color:var(--rv-faint);line-height:1.6}
.sh .sh-note{border-left:3px solid var(--rv-signal);padding:4px 18px;margin:24px 0 32px}
.sh .sh-card{border:1px solid var(--rv-line);border-radius:10px;padding:22px;background:var(--rv-panel);margin:12px 0;scroll-margin-top:20px;overflow-wrap:anywhere}
.sh .sh-card:target{outline:2px solid var(--rv-signal);outline-offset:3px}
.sh .sh-primary{border-color:var(--rv-signal)}
.sh .sh-cardhead{display:flex;gap:12px;align-items:center;justify-content:space-between;margin-bottom:18px}
.sh .sh-copy{flex:none;border:1px solid var(--rv-line);border-radius:5px;background:var(--rv-panel);color:var(--rv-faint);font:11px ui-monospace,monospace;padding:4px 8px;cursor:pointer}
.sh .sh-context{font:500 22px "Iowan Old Style",Palatino,Georgia,serif;margin:34px 0 12px}
.sh .sh-notice{font:11px ui-monospace,monospace;color:var(--rv-faint);line-height:1.6;margin:14px 0 0}
.sh .sh-inspect{margin:32px 0;color:var(--rv-faint);font:12px ui-monospace,monospace}
.sh summary{cursor:pointer}.sh pre{max-height:420px;overflow:auto;padding:16px;background:var(--rv-soft);font-size:11px;line-height:1.5}
@media(max-width:560px){.sh h1{font-size:28px}.sh .sh-card{padding:16px}.sh .sh-cardhead{align-items:flex-start}}
`;

const label = (item: Item) => ({ tool_call: "Tool arguments", tool_result: "Tool result", error: "Recorded error", thinking: "Recorded reasoning", data: "Recorded data", source_reference: "Source reference" }[item.kind] ?? { user: "Human input", assistant: "Agent response", system: "Provided context", tool: "Tool result" }[item.role] ?? item.role);
const spanText = (run: Run, id: string) => {
  const span = run.spans.find(s => s.id === id) as { input?: { messages?: { content?: { type: string; text?: string }[] }[] } } | undefined;
  return (span?.input?.messages ?? []).flatMap(m => m.content ?? []).filter(p => p.type === "text" && typeof p.text === "string").map(p => p.text!).join("\n\n");
};

function ExcerptCard({ run, item, primary, renderText }: { run: Run; item: Item; primary: boolean; renderText: (text: string) => ReactNode }) {
  const [copy, setCopy] = useState("Copy link");
  const copyLink = async () => {
    try {
      await navigator.clipboard.writeText(withAnchor(location.href, item.id)); setCopy("Copied");
    } catch { setCopy("Copy failed"); }
  };
  return <article className={`sh-card${primary ? " sh-primary" : ""}`} id={`span=${item.id}`}>
    <div className="sh-cardhead"><span className="sh-meta">{primary ? "Start here · " : ""}{label(item)}{item.passage ? " · selected passage" : ""}</span><button className="sh-copy" onClick={copyLink}>{copy}</button></div>
    {renderText(item.kind === "source_reference" ? spanText(run, item.id).replace(/^(\s*)\[/gm, "$1\\[") : spanText(run, item.id))}
    {item.prompt_missing && <p className="sh-notice">Human input not included.</p>}
    {item.tool_call_missing && <p className="sh-notice">The corresponding tool arguments are not included.</p>}
    {item.omitted_before && !primary && <p className="sh-notice">Other session material preceded this excerpt and is omitted.</p>}
  </article>;
}

export function ShareView({ run, share, renderText }: { run: Run; share: Share; renderText: (text: string, definitions?: string) => ReactNode }) {
  const primary = share.items.find(i => i.id === share.primary_id) ?? share.items[0];
  const definitions = share.items.filter(i => i.kind === "source_reference").map(i => spanText(run, i.id)).join("\n");
  const renderIncluded = (text: string) => renderText(text, definitions);
  useEffect(() => {
    const scroll = () => { const id = new URLSearchParams(location.hash.slice(1)).get("span"); if (id && /^[\w.-]+$/.test(id)) document.getElementById(`span=${id}`)?.scrollIntoView({ block: "start" }); };
    scroll(); window.addEventListener("hashchange", scroll); return () => window.removeEventListener("hashchange", scroll);
  }, [run]);
  return <div className="rv sh">
    <style>{CSS}</style>
    <p className="sh-meta">Session excerpt</p>
    <h1>{run.name ?? "Shared session excerpt"}</h1>
    <p className="sh-intro">Selected material from a session. Other content is not included.</p>
    {share.references_unavailable && <p className="sh-notice">Some cited source references were unavailable in the captured material.</p>}
    {share.note_id && <aside className="sh-note"><p className="sh-meta">Author note</p>{renderText(spanText(run, share.note_id))}</aside>}
    {primary && <ExcerptCard run={run} item={primary} primary renderText={renderIncluded} />}
    {share.items.length > 1 && <h2 className="sh-context">Relevant context <span className="sh-meta">· in session order</span></h2>}
    {share.items.filter(item => item.id !== primary?.id).map(item => <ExcerptCard key={item.id} run={run} item={item} primary={false} renderText={renderIncluded} />)}
    <p className="sh-notice">{share.original_fidelity === "exact" ? "The original session was captured exactly. " : share.original_fidelity === "reconstructed" ? "The original session was reconstructed from agent history. " : ""}This excerpt contains selected text and any author note above.</p>
    <details className="sh-inspect"><summary>Inspect included data</summary><pre>{JSON.stringify(run, null, 2)}</pre></details>
  </div>;
}
