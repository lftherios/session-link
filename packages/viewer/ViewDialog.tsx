import { useEffect, useRef, useState } from "react";
import type { Run } from "@session-link/format";
import { DocumentText } from "./DocumentText";
import { shortText } from "./session-model";
import { belongsTo, unitLabel, unitsFor, viewRequest, type SavedView, type ViewDraft, type ViewItem, type ViewUnit } from "./view-draft";

type Props = {
  source: string; title: string; prefixes: string[]; primary?: string; activity: string[];
  explicit: boolean; reconcile: boolean; intent: "share" | "annotate" | "edit";
  draft: ViewDraft | null; onDraft: (draft: ViewDraft) => void; close: () => void;
};

export function ViewDialog({ source, title, prefixes, primary, activity, explicit, reconcile, intent, draft, onDraft, close }: Props) {
  const ref = useRef<HTMLDialogElement>(null), note = useRef<HTMLTextAreaElement>(null);
  const [units, setUnits] = useState<ViewUnit[] | null>(null), [savedViews, setSavedViews] = useState<SavedView[]>([]);
  const [error, setError] = useState(""), [busy, setBusy] = useState(false), [loading, setLoading] = useState(true);
  const [saved, setSaved] = useState<{ url: string; document: Run; fingerprint: string } | null>(null);
  const [details, setDetails] = useState(intent !== "share"), [passage, setPassage] = useState("");
  const [range, setRange] = useState<{ start: number; end: number } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [omitted, setOmitted] = useState(0);
  const [missingReasoning, setMissingReasoning] = useState(false);
  useEffect(() => { ref.current?.showModal(); }, []);
  useEffect(() => {
    let alive = true;
    setLoading(true); setError("");
    viewRequest<{ catalog: { units: ViewUnit[] }; views: SavedView[] }>(`/api/compose/${source}`).then(data => {
      if (!alive) return;
      const catalog = data.catalog.units ?? [];
      setUnits(catalog); setSavedViews(data.views ?? []);
      const unavailable = catalog.filter(unit => unit.unavailable && prefixes.some(prefix => belongsTo(unit, prefix)));
      setOmitted(unavailable.filter(unit => unit.kind !== "thinking").length);
      setMissingReasoning(unavailable.some(unit => unit.kind === "thinking"));
      if (!draft || reconcile) {
        // A normal exchange opens with its human input and text response.
        // Explicit selections may additionally include activity and data.
        const selected = unitsFor(catalog, prefixes).filter(unit => explicit || unit.kind === "text" || unit.kind === "error");
        onDraft({ title: draft?.title ?? title, note: draft?.note ?? "", items: selected.map(unit => draft?.items.find(item => item.id === unit.id) ?? { id: unit.id }), primary: selected.some(unit => unit.id === draft?.primary) ? draft!.primary : selected.find(unit => primary && belongsTo(unit, primary))?.id ?? selected[0]?.id ?? "" });
      }
      setLoading(false);
    }).catch(error => { if (alive) { setError(error.message); setLoading(false); } });
    return () => { alive = false; };
  }, [source, attempt]);
  useEffect(() => { if (intent === "annotate" && units) note.current?.focus(); }, [intent, units]);

  const change = (value: ViewDraft) => { setError(""); onDraft(value); };
  const replace = (id: string, selection?: ViewItem) => {
    if (!draft) return;
    const items = draft.items.filter(item => item.id !== id); if (selection) items.push(selection);
    change({ ...draft, items, primary: items.some(item => item.id === draft.primary) ? draft.primary : items[0]?.id ?? "" });
  };
  const include = (ids: string[]) => { if (draft) change({ ...draft, items: [...draft.items.filter(item => !ids.includes(item.id)), ...ids.map(id => ({ id }))] }); };
  const save = async (preview: boolean) => {
    if (!draft) return;
    setBusy(true); setError("");
    try {
      const result = await viewRequest<{ url: string; document: Run }>(`/api/export/${source}`, "POST", draft);
      setSaved({ ...result, fingerprint: JSON.stringify(draft) });
      setSavedViews(old => [{ url: result.url, title: draft.title }, ...old.filter(view => view.url !== result.url)]);
      if (preview) location.assign(result.url);
    } catch (error) { setError(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(false); }
  };
  const download = () => {
    if (!saved) return;
    const url = URL.createObjectURL(new Blob([JSON.stringify(saved.document, null, 2)], { type: "application/json" }));
    const anchor = document.createElement("a"); anchor.href = url; anchor.download = "session-view.json"; anchor.click();
    setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  const selected = (units ?? []).filter(unit => draft?.items.some(item => item.id === unit.id));
  const extras = unitsFor(units ?? [], activity).filter(unit => !draft?.items.some(item => item.id === unit.id));
  const readRange = (el: HTMLTextAreaElement) => setRange(el.selectionEnd > el.selectionStart ? { start: el.selectionStart, end: el.selectionEnd } : null);
  const isSaved = saved?.fingerprint === JSON.stringify(draft);
  return <dialog className="sv-dialog sv-share-dialog" aria-label="Share this view" ref={ref} onCancel={event => { event.preventDefault(); if (!busy) close(); }}>
    <div className="sv-dialog-head"><h2>{intent === "annotate" ? "Annotate selection" : intent === "edit" ? "Edit view" : "Share this view"}</h2><button className="sv-quiet" aria-label="Close share panel" disabled={busy} onClick={close}>✕</button></div>
    <p className="sv-dialog-intro">{explicit ? "Your selected material" : "The human input and agent response in this view"}. Review what’s included, then save a local copy.</p>
    {error && <p className="sv-notice sv-error" role="alert">{error}{!units && <button onClick={() => setAttempt(value => value + 1)}>Try again</button>}</p>}
    {loading ? <p role="status" className="sv-meta">Loading view…</p> : units && draft && <fieldset disabled={busy}>
      <details className="sv-author-fields" open={details} onToggle={event => setDetails(event.currentTarget.open)}>
        <summary>Title and annotation{draft.note ? " · note added" : ""}</summary>
        <label>View title<input aria-label="View title" value={draft.title} maxLength={256} onChange={event => change({ ...draft, title: event.target.value })} /></label>
        <label>Your annotation<textarea ref={note} aria-label="Your annotation" placeholder="What should your colleague look at or help with?" value={draft.note} maxLength={10000} onChange={event => change({ ...draft, note: event.target.value })} /></label>
        <p className="sv-meta">Shown separately from the recorded session.</p>
      </details>
      <div className="sv-included-heading"><span className="sv-meta">{selected.length} {selected.length === 1 ? "piece" : "pieces"} included</span>{extras.length > 0 && <button className="sv-quiet" onClick={() => include(extras.map(unit => unit.id))}>Include agent activity ({extras.length})</button>}</div>
      {omitted > 0 && <p className="sv-notice">{omitted} image, attachment or other unsupported {omitted === 1 ? "item is" : "items are"} unavailable for this text view.</p>}
      {missingReasoning && <p className="sv-notice">Some reasoning text is unavailable and cannot be included.</p>}
      <div className="sv-included">
        {selected.map(unit => {
          const item = draft.items.find(item => item.id === unit.id)!;
          const text = item.start == null ? unit.text : unit.text.slice(item.start, item.end);
          const missingPrompts = (unit.prompt_ids ?? []).filter(id => !draft.items.some(item => item.id === id && item.start == null));
          return <details className="sv-included-item" key={unit.id} open={passage === unit.id || undefined}>
            <summary><span className="sv-meta">{unitLabel(unit)}{item.start != null ? " · passage" : ""}{draft.primary === unit.id ? " · opens first" : ""}</span><span>{shortText(text.split(/\n\s*\n/)[0].replace(/^\s{0,3}#{1,6}\s+/, ""), 150)}</span></summary>
            <div className="sv-included-body"><DocumentText text={text} />
              <div className="sv-item-actions"><button onClick={() => change({ ...draft, primary: unit.id })} disabled={draft.primary === unit.id}>Start here</button><button onClick={() => { setPassage(unit.id); setRange(null); }}>Select passage</button>{item.start != null && <button onClick={() => replace(unit.id, { id: unit.id })}>Use full text</button>}<button onClick={() => replace(unit.id)}>Remove</button>{missingPrompts.length > 0 && <button onClick={() => include(missingPrompts)}>Include human input</button>}</div>
              {unit.prompt_incomplete && <p className="sv-meta">Some human input was not captured.</p>}
              {passage === unit.id && <div className="sv-passage-editor"><p className="sv-meta">Highlight the exact source passage to include.</p><textarea className="sv-passage" aria-label="Select source passage" value={unit.text} readOnly onSelect={event => readRange(event.currentTarget)} onMouseUp={event => readRange(event.currentTarget)} onKeyUp={event => readRange(event.currentTarget)} /><button disabled={!range} onClick={() => { if (range) { replace(unit.id, { id: unit.id, ...range }); setPassage(""); } }}>Use selected passage</button></div>}
            </div>
          </details>;
        })}
        {selected.length === 0 && <p className="sv-notice">No supported content is included. Return to the session to select material.</p>}
      </div>
      {units.some(unit => unit.kind === "source_reference" && !draft.items.some(item => item.id === unit.id)) && <details className="sv-reference-options"><summary>Available source references</summary>{units.filter(unit => unit.kind === "source_reference" && !draft.items.some(item => item.id === unit.id)).map(unit => <label key={unit.id}><input type="checkbox" onChange={() => include([unit.id])} /><span>{unit.text}</span></label>)}</details>}
      <div className="sv-share-footer"><p className="sv-meta">Only the included material, title and annotation enter the saved view. Link publishing is not available in this local build.</p><div className="sv-item-actions"><button disabled={!selected.length || !draft.title.trim()} onClick={() => save(true)}>Preview view</button><button className="sv-primary" disabled={!selected.length || !draft.title.trim() || isSaved} onClick={() => save(false)}>{busy ? "Saving…" : isSaved ? "Saved locally" : "Save locally"}</button></div></div>
      {saved && <div className="sv-saved-notice" role="status"><span>{isSaved ? "View saved locally." : "An earlier version is saved. Save again to keep these changes."}</span><a href={saved.url}>Open saved view</a><button className="sv-quiet" onClick={download}>Download view</button></div>}
    </fieldset>}
    {savedViews.length > 0 && <details className="sv-saved-views"><summary>Saved views · {savedViews.length}</summary>{savedViews.map(view => <a key={view.url} href={view.url}>{view.title}</a>)}</details>}
  </dialog>;
}
