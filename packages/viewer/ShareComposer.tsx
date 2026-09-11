import { useEffect, useMemo, useRef, useState } from "react";
import { ExcerptText, RV_CSS } from "./RunViewer";
import { meaningfulTitle, shortText } from "./session-model";

type Unit = { id: string; span_id: string; role: string; kind: string; text: string; prompt_ids?: string[]; prompt_incomplete?: boolean; unavailable?: boolean };
type Selection = { id: string; start?: number; end?: number };
type Draft = { title: string; note: string; items: Selection[]; primary: string };
type Loaded = { source_title?: string; catalog: { units: Unit[] }; revision: number; draft: Draft };

const CSS = `
.sc{font-family:system-ui,sans-serif;color:var(--rv-ink)}
.sc a{color:var(--rv-signal)}.sc h1{font:500 32px "Iowan Old Style",Palatino,Georgia,serif;margin:18px 0 8px}
.sc .sc-meta{font:11px ui-monospace,monospace;color:var(--rv-faint);letter-spacing:.07em;text-transform:uppercase;overflow-wrap:anywhere}
.sc .sc-intro{color:var(--rv-faint);font-size:14px;line-height:1.6;max-width:680px}
.sc .sc-layout{display:grid;grid-template-columns:minmax(0,1fr) 300px;gap:24px;margin-top:28px;align-items:start}
.sc .sc-side{position:sticky;top:20px;border:1px solid var(--rv-line);border-radius:10px;background:var(--rv-panel);padding:20px}
.sc label.sc-field{display:grid;gap:8px;margin-bottom:18px;font-size:13px;font-weight:600}
.sc input[type=text],.sc input[type=search],.sc textarea{width:100%;box-sizing:border-box;background:var(--rv-panel);color:var(--rv-ink);border:1px solid var(--rv-line);border-radius:6px;font:13px system-ui,sans-serif;padding:10px;line-height:1.5}
.sc textarea{resize:vertical;min-height:100px}.sc .sc-passage{font:12px ui-monospace,monospace;min-height:180px;max-height:440px}
.sc input[type=checkbox],.sc input[type=radio]{accent-color:var(--rv-signal);width:16px;height:16px;flex:none}
.sc button{border:1px solid var(--rv-line);border-radius:6px;background:var(--rv-panel);color:var(--rv-ink);padding:7px 10px;font:11px ui-monospace,monospace;cursor:pointer}
.sc button:disabled{opacity:.5;cursor:default}.sc .sc-primary{background:var(--rv-signal);border-color:var(--rv-signal);color:var(--rv-paper);font-weight:600;width:100%;padding:12px}
.sc .sc-card{background:var(--rv-panel);border:1px solid var(--rv-line);border-radius:10px;margin:12px 0;padding:18px;overflow-wrap:anywhere}
.sc .sc-card.selected{border-color:var(--rv-signal)}.sc .sc-cardhead{display:flex;align-items:center;justify-content:space-between;gap:10px;margin-bottom:14px}
.sc .sc-check{display:flex;gap:9px;align-items:center;font:11px ui-monospace,monospace;cursor:pointer}
.sc .sc-actions{display:flex;gap:10px;flex-wrap:wrap;align-items:center;margin-top:16px}
.sc .sc-note{color:var(--rv-faint);font:11px ui-monospace,monospace;line-height:1.65;margin:12px 0}
.sc .sc-error{color:var(--rv-error);white-space:pre-wrap}.sc .sc-tools{display:flex;align-items:center;gap:12px;flex-wrap:wrap}.sc .sc-tools input{flex:1;min-width:160px}
.sc fieldset{border:0;padding:0;margin:0;min-width:0}.sc .sc-empty{padding:32px 12px;color:var(--rv-faint);text-align:center;font-size:13px}
.sc :where(input,textarea,button,a):focus-visible{outline:2px solid var(--rv-signal);outline-offset:3px}
@media(max-width:820px){.sc .sc-layout{grid-template-columns:minmax(0,1fr)}.sc .sc-side{position:static;grid-row:1}.sc h1{font-size:28px}}
`;

const kindLabel = (u: Unit) => ({ tool_call: "Tool arguments", tool_result: "Tool result", thinking: "Reasoning", error: "Recorded error", data: "Recorded data", source_reference: "Source reference", unavailable: "Unavailable content" }[u.kind] ?? u.role);

function UnitCard({ unit, selection, primary, units, selections, initialPassage, onSelect, onPrimary, onPrompt }: {
  unit: Unit; selection?: Selection; primary: boolean; units: Unit[]; selections: Selection[];
  initialPassage?: boolean;
  onSelect: (selection?: Selection) => void; onPrimary: () => void; onPrompt: () => void;
}) {
  const [passage, setPassage] = useState(initialPassage ?? false);
  const [range, setRange] = useState<{ start: number; end: number } | null>(null);
  const readRange = (el: HTMLTextAreaElement) => setRange(el.selectionEnd > el.selectionStart ? { start: el.selectionStart, end: el.selectionEnd } : null);
  const clipped = selection?.start != null;
  const shown = clipped ? unit.text.slice(selection.start, selection.end) : unit.text;
  const prompts = unit.prompt_ids ?? [];
  const missingPrompt = prompts.some(id => !selections.some(s => s.id === id && s.start == null));
  return <article className={`sc-card${selection ? " selected" : ""}`} data-unit={unit.id}>
    <div className="sc-cardhead">
      <label className="sc-check"><input type="checkbox" checked={!!selection} disabled={unit.unavailable} onChange={e => onSelect(e.target.checked ? { id: unit.id } : undefined)} aria-label={`Include ${kindLabel(unit)} ${unit.id}`} /><span>{kindLabel(unit)}{clipped ? " · selected passage" : ""}</span></label>
      {selection && <label className="sc-check"><input type="radio" name="primary" checked={primary} onChange={onPrimary} />Start here</label>}
    </div>
    <ExcerptText text={shown} />
    {!unit.unavailable && <div className="sc-actions">
      <button onClick={() => { setPassage(v => !v); setRange(null); }}>{passage ? "Close passage selector" : clipped ? "Change passage" : "Select a passage"}</button>
      {clipped && <button onClick={() => onSelect({ id: unit.id })}>Use full text</button>}
      {selection && missingPrompt && <button onClick={onPrompt}>Include human input</button>}
      <span className="sc-meta">Source item {units.indexOf(unit) + 1}</span>
    </div>}
    {selection && (unit.role === "assistant" || unit.role === "tool" || unit.kind === "error") && (prompts.length === 0 || unit.prompt_incomplete) && <p className="sc-note">{unit.prompt_incomplete ? "Some human input is unavailable for selection." : "Human input unavailable in this session."}</p>}
    {passage && <div style={{ marginTop: 16 }}>
      <p className="sc-note">Select the exact passage below, then include it. Source text stays unchanged.</p>
      <textarea className="sc-passage" readOnly value={unit.text} aria-label={`Select passage from ${unit.id}`} onSelect={e => readRange(e.currentTarget)} onMouseUp={e => readRange(e.currentTarget)} onKeyUp={e => readRange(e.currentTarget)} />
      <button disabled={!range} onClick={() => { if (range) { onSelect({ id: unit.id, ...range }); setPassage(false); } }}>Use selected passage</button>
    </div>}
  </article>;
}

async function request<T>(path: string, method = "GET", body?: unknown): Promise<T> {
  const response = await fetch(path, { method, headers: { "x-slink": "1", "content-type": "application/json" }, body: body === undefined ? undefined : JSON.stringify(body) });
  const data = await response.json();
  if (!response.ok) throw new Error(data.error?.message ?? "Could not save the excerpt");
  return data as T;
}

export function ShareComposer({ source }: { source: string }) {
  const [loaded, setLoaded] = useState<Loaded | null>(null);
  const [draft, setDraft] = useState<Draft | null>(null);
  const [status, setStatus] = useState("Loading source…");
  const [error, setError] = useState("");
  const [query, setQuery] = useState("");
  const [onlySelected, setOnlySelected] = useState(false);
  const [shown, setShown] = useState(50);
  const [working, setWorking] = useState(false);
  const [incoming, setIncoming] = useState<Selection[]>([]);
  const [passageUnit, setPassageUnit] = useState("");
  const revision = useRef(0), saved = useRef("");
  const latest = useRef<Draft | null>(null);
  const queue = useRef<Promise<void>>(Promise.resolve());
  latest.current = draft;
  useEffect(() => {
    let alive = true;
    const view = new URLSearchParams(location.search).get("view");
    request<Loaded>(`/api/compose/${source}${view ? `?view=${encodeURIComponent(view)}` : ""}`).then(data => {
      if (!alive) return;
      revision.current = data.revision;
      data.draft.items ??= []; data.draft.primary ??= "";
      saved.current = JSON.stringify(data.draft);
      const params = new URLSearchParams(location.search), from = params.get("from");
      const selected = from ? data.catalog.units.filter(unit => !unit.unavailable && ((unit.kind === "text" && unit.role === "assistant" && unit.id.startsWith(from + "-") && !unit.id.includes("-ref-")) || (unit.kind === "error" && unit.id === from))).map(unit => ({ id: unit.id })) : [];
      let initial = data.draft;
      const suggestedTitle = meaningfulTitle(data.source_title) ? data.source_title! : shortText(data.catalog.units.find(unit => unit.role === "user" && unit.kind === "text" && !/^\s*</.test(unit.text))?.text ?? "Session excerpt", 90);
      if (from && selected.length === 0) setError("This agent response is unavailable for selection. Choose the material below.");
      if (selected.length && initial.items.length === 0) {
        initial = { ...initial, items: selected, primary: selected[0].id, title: initial.title === "Shared session excerpt" ? suggestedTitle.slice(0, 256) : initial.title };
        setOnlySelected(true);
      } else if (selected.length && !selected.every(item => initial.items.some(saved => saved.id === item.id))) {
        setIncoming(selected);
      } else if (selected.length) setOnlySelected(true);
      if (params.get("passage") === "1" && selected.length) setPassageUnit(selected[0].id);
      setLoaded(data); setDraft(initial); setStatus("Saved locally");
    }).catch(e => { if (alive) setError(String(e.message)); });
    return () => { alive = false; };
  }, [source]);

  const save = (value: Draft) => {
    const task = queue.current.then(async () => {
      const fingerprint = JSON.stringify(value);
      if (saved.current === fingerprint) return;
      setStatus("Saving…");
      const result = await request<{ revision: number }>(`/api/draft/${source}`, "PUT", { revision: revision.current, draft: value });
      revision.current = result.revision; saved.current = fingerprint;
      if (JSON.stringify(latest.current) === fingerprint) setStatus("Saved locally");
    });
    queue.current = task.catch(() => {});
    return task;
  };
  useEffect(() => {
    if (!draft || saved.current === JSON.stringify(draft)) return;
    setStatus("Unsaved changes");
    const timer = window.setTimeout(() => { save(draft).catch(e => { setError(e.message); setStatus("Not saved"); }); }, 350);
    return () => window.clearTimeout(timer);
  }, [draft]);
  useEffect(() => {
    const guard = (event: BeforeUnloadEvent) => { if (latest.current && saved.current !== JSON.stringify(latest.current)) { event.preventDefault(); event.returnValue = ""; } };
    window.addEventListener("beforeunload", guard); return () => window.removeEventListener("beforeunload", guard);
  }, []);

  const units = loaded?.catalog.units ?? [];
  const selectedIDs = useMemo(() => new Set(draft?.items.map(item => item.id)), [draft]);
  const filtered = useMemo(() => units.filter(unit => (!onlySelected || selectedIDs.has(unit.id)) && `${kindLabel(unit)} ${unit.text}`.toLowerCase().includes(query.trim().toLowerCase())), [units, onlySelected, selectedIDs, query]);
  const change = (next: Draft) => { setError(""); setDraft(next); };
  const preview = async () => {
    if (!draft) return;
    setWorking(true); setError("");
    try { await save(draft); const result = await request<{ url: string }>(`/api/export/${source}`, "POST", draft); location.assign(result.url); }
    catch (e) { setError(e instanceof Error ? e.message : String(e)); setWorking(false); }
  };

  return <div className="rv sc"><style>{RV_CSS + CSS}</style>
    <a className="sc-meta" href={`/p/${source}`}>← Back to session</a>
    <h1>Share the useful part.</h1>
    <p className="sc-intro">Choose a finding, an agent response, or the evidence behind a question. Include the human input behind it and add an author note for your colleague.</p>
    {error && <p className="sc-note sc-error" role="alert">{error}</p>}
    {incoming.length > 0 && draft && <div className="sc-card"><p>You already have a saved draft for this session.</p><button onClick={() => { const ids = new Set(incoming.map(item => item.id)); change({ ...draft, items: [...draft.items.filter(item => !ids.has(item.id)), ...incoming], primary: incoming[0].id }); setIncoming([]); setOnlySelected(true); }}>Add this agent response to the draft</button></div>}
    {!loaded || !draft ? <p className="sc-note">{status}</p> : <fieldset disabled={working}><div className="sc-layout">
      <section aria-label="Source material">
        <p className="sc-meta">Source · {loaded.source_title ?? "Session"}</p>
        <div className="sc-tools"><input type="search" placeholder="Find human input, an agent response, or activity…" aria-label="Search source material" value={query} onChange={e => { setQuery(e.target.value); setShown(50); }} /><label className="sc-check"><input type="checkbox" checked={onlySelected} onChange={e => { setOnlySelected(e.target.checked); setShown(50); }} />Included only</label></div>
        {filtered.slice(0, shown).map(unit => <UnitCard key={unit.id} unit={unit} units={units} selection={draft.items.find(item => item.id === unit.id)} selections={draft.items} primary={draft.primary === unit.id} initialPassage={unit.id === passageUnit}
          onSelect={selection => {
            const items = draft.items.filter(item => item.id !== unit.id); if (selection) items.push(selection);
            const primary = items.some(item => item.id === draft.primary) ? draft.primary : (selection?.id ?? items[0]?.id ?? "");
            change({ ...draft, items, primary });
          }}
          onPrimary={() => change({ ...draft, primary: unit.id })}
          onPrompt={() => { const ids = unit.prompt_ids ?? []; change({ ...draft, items: [...draft.items.filter(item => !ids.includes(item.id)), ...ids.map(id => ({ id }))] }); }} />)}
        {filtered.length === 0 && <p className="sc-empty">{units.length === 0 ? "No selectable text was captured in this session." : "No material matches this filter."}</p>}
        {filtered.length > shown && <button onClick={() => setShown(n => n + 50)}>Show more ({filtered.length - shown} remaining)</button>}
      </section>
      <aside className="sc-side" aria-label="Excerpt details">
        <label className="sc-field">Title<input type="text" value={draft.title} maxLength={256} onChange={e => change({ ...draft, title: e.target.value })} /></label>
        <label className="sc-field">Author note <span className="sc-meta">Optional</span><textarea value={draft.note} maxLength={10000} placeholder="What should your colleague look at or help with?" onChange={e => change({ ...draft, note: e.target.value })} /></label>
        <p className="sc-note">{draft.items.length} {draft.items.length === 1 ? "piece" : "pieces"} included. Choose “Start here” on the main finding or question.</p>
        <button className="sc-primary" disabled={working || draft.items.length === 0 || !draft.title.trim()} onClick={preview}>{working ? "Preparing preview…" : "Preview excerpt"}</button>
        <p className="sc-note" role="status">{status}</p>
        <p className="sc-note">Only your selected text, title, and author note enter the excerpt. Preview and download are available locally.</p>
      </aside>
    </div></fieldset>}
  </div>;
}
