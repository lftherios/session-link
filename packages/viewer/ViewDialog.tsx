import { useEffect, useRef, useState } from "react";
import type { Run } from "@session-link/format";
import { PublishPanel, type Published } from "./PublishPanel";
import { DocumentText } from "./DocumentText";
import { previewText } from "./session-model";
import { belongsTo, unitLabel, unitsFor, viewRequest, type SavedView, type ViewDraft, type ViewItem, type ViewUnit } from "./view-draft";

type Props = {
  source: string; title: string; prefixes: string[]; primary?: string; activity: string[];
  explicit: boolean; reconcile: boolean; intent: "share" | "annotate" | "edit";
  draft: ViewDraft | null; onDraft: (draft: ViewDraft) => void; close: () => void;
};

export function ViewDialog({ source, title, prefixes, primary, activity, explicit, reconcile, intent, draft, onDraft, close }: Props) {
  const ref = useRef<HTMLDialogElement>(null), note = useRef<HTMLTextAreaElement>(null), publishBox = useRef<HTMLDivElement>(null);
  const [units, setUnits] = useState<ViewUnit[] | null>(null), [savedViews, setSavedViews] = useState<SavedView[]>([]);
  const [error, setError] = useState(""), [busy, setBusy] = useState<"" | "preview" | "save" | "publish">(""), [loading, setLoading] = useState(true);
  const [saved, setSaved] = useState<{ url: string; document: Run; fingerprint: string } | null>(null);
  const [details, setDetails] = useState(intent !== "share"), [passage, setPassage] = useState("");
  const [range, setRange] = useState<{ start: number; end: number } | null>(null);
  const [attempt, setAttempt] = useState(0);
  const [publishing, setPublishing] = useState(false);
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
  // Publishing continues below the actions; take the reader there.
  useEffect(() => {
    const panel = publishing ? publishBox.current?.querySelector<HTMLElement>(".sl-publish") : null;
    panel?.focus({ preventScroll: true }); panel?.scrollIntoView({ block: "nearest" });
  }, [publishing, saved]);
  // Close the modal before the reader refocuses the control that opened it.
  const done = () => { ref.current?.close(); close(); };

  const change = (value: ViewDraft) => { setPublishing(false); setError(""); onDraft(value); };
  const replace = (id: string, selection?: ViewItem) => {
    if (!draft) return;
    const items = draft.items.filter(item => item.id !== id); if (selection) items.push(selection);
    change({ ...draft, items, primary: items.some(item => item.id === draft.primary) ? draft.primary : items[0]?.id ?? "" });
  };
  const include = (ids: string[]) => { if (draft) change({ ...draft, items: [...draft.items.filter(item => !ids.includes(item.id)), ...ids.map(id => ({ id }))] }); };
  const save = async (preview: boolean, publish = false) => {
    if (!draft) return;
    // The preview opens beside this panel, so the preparation stays in place.
    // The tab is created during the click, which popup blockers allow.
    const tab = preview ? window.open("about:blank", "_blank") : null;
    if (tab) tab.opener = null;
    setBusy(preview ? "preview" : publish ? "publish" : "save"); setError("");
    try {
      const result = await viewRequest<{ url: string; document: Run }>(`/api/export/${source}`, "POST", draft);
      setSaved({ ...result, fingerprint: JSON.stringify(draft) });
      setPublishing(publish);
      setSavedViews(old => [{ ...old.find(view => view.url === result.url), url: result.url, title: draft.title }, ...old.filter(view => view.url !== result.url)]);
      if (preview) { if (tab) tab.location.href = result.url; else location.assign(result.url); }
    } catch (error) { tab?.close(); setError(error instanceof Error ? error.message : String(error)); }
    finally { setBusy(""); }
  };
  const published = (view: string, result: Published) => setSavedViews(old => old.map(item => item.url === view ? { ...item, published: { ...result, published_at: Date.now() } } : item));
  // Names the included piece that holds text the secret scan stopped on.
  const locate = (head: string) => {
    const unit = selected.find(unit => { const item = draft?.items.find(item => item.id === unit.id); return (item?.start == null ? unit.text : unit.text.slice(item.start, item.end)).includes(head); });
    return unit && `The included ${unitLabel(unit).toLowerCase()} “${previewText(unit.text, 60)}”`;
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
  const receipt = saved && savedViews.find(view => view.url === saved.url)?.published;
  return <dialog className="sv-dialog sv-share-dialog" aria-labelledby="sv-share-title" ref={ref} onCancel={event => { event.preventDefault(); if (!busy) done(); }}>
    <div className="sv-dialog-head"><h2 id="sv-share-title">{intent === "annotate" ? "Comment and share" : intent === "edit" ? "Edit view" : "Share this view"}</h2><button className="sv-quiet" aria-label="Close share panel" disabled={!!busy} onClick={done}>✕</button></div>
    <p className="sv-dialog-intro">{explicit ? "Your selected material" : "The human input and agent response in this view"}. Review what’s included, then publish an encrypted link or save a local copy.</p>
    {error && <p className="sv-notice sv-error" role="alert">{error}{!units && <button onClick={() => setAttempt(value => value + 1)}>Try again</button>}</p>}
    {loading ? <p role="status" className="sv-meta">Loading view…</p> : units && draft && <fieldset disabled={!!busy}>
      <details className="sv-author-fields" open={details} onToggle={event => setDetails(event.currentTarget.open)}>
        <summary>Title and comment{draft.note ? " · comment added" : ""}</summary>
        <label>View title<input aria-label="View title" value={draft.title} maxLength={256} onChange={event => change({ ...draft, title: event.target.value })} /></label>
        <label>Your comment<textarea ref={note} aria-label="Your comment" placeholder="What should your colleague look at or help with?" value={draft.note} maxLength={10000} onChange={event => change({ ...draft, note: event.target.value })} /></label>
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
            <summary><span className="sv-meta">{unitLabel(unit)}{item.start != null ? " · passage" : ""}{draft.primary === unit.id ? " · opens first" : ""}</span><span>{previewText(text.split(/\n\s*\n/)[0], 150)}</span></summary>
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
      <div className="sv-share-footer"><p className="sv-meta">Only the included material, title and comment enter the saved view. The rest of your session stays local.</p><div className="sv-item-actions"><button disabled={!selected.length || !draft.title.trim()} onClick={() => save(true)}>{busy === "preview" ? "Opening preview…" : "Preview view"}</button><button disabled={!selected.length || !draft.title.trim() || isSaved} onClick={() => save(false)}>{busy === "save" ? "Saving…" : isSaved ? "Saved locally" : "Save locally"}</button><button className="sv-primary" disabled={!selected.length || !draft.title.trim()} onClick={() => save(false, true)}>{busy === "publish" ? "Preparing…" : "Publish link"}</button></div>{!draft.title.trim() && selected.length > 0 && <p className="sv-meta">Add a view title to save or publish.</p>}</div>
      {saved && <div className="sv-saved-notice" role="status"><span>{isSaved ? "View saved locally." : "An earlier version is saved. Save again to keep these changes."}</span><a href={saved.url}>Open saved view</a><button className="sv-quiet" onClick={download}>Download view</button></div>}
    </fieldset>}
    {publishing && saved && isSaved ? <div ref={publishBox}><PublishPanel key={saved.url} endpoint={`/api/publish-preview/${saved.url.split("/").pop()}`} title={draft?.title ?? title} ready onPublished={result => published(saved.url, result)} locate={locate} /></div>
      : receipt && <div className="sv-saved-notice sv-published" role="status"><span>{isSaved ? "Published." : "Published an earlier version. Publish again to share your changes."}</span><PublishedLink published={receipt} /></div>}
    {savedViews.length > 0 && <details className="sv-saved-views"><summary>Saved views · {savedViews.length}{savedViews.some(view => view.published) ? ` · ${savedViews.filter(view => view.published).length} published` : ""}</summary>{savedViews.map(view => <div className="sv-saved-view" key={view.url}><a href={view.url}>{view.title}</a>{view.published && <PublishedLink published={view.published} />}</div>)}</details>}
  </dialog>;
}

// A published view's link, or for named shares where to manage access.
function PublishedLink({ published }: { published: NonNullable<SavedView["published"]> }) {
  const [copied, setCopied] = useState("");
  if (published.recipients) return <a className="sv-published-link" href="/shared">Shared with {published.recipients} {published.recipients === 1 ? "person" : "people"}</a>;
  return <span className="sv-published-link"><span className="sv-meta">Published link</span><button className="sv-quiet" onClick={async () => { try { await navigator.clipboard.writeText(published.url); setCopied("Copied"); } catch { setCopied("Could not copy"); } }}>{copied || "Copy link"}</button></span>;
}
