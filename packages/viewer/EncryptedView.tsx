"use client";
import { useEffect, useState } from "react";
import type { Run } from "@session-link/format";
import { RunViewer } from "./RunViewer";
import { readCiphertext, openDocument } from "./sealed-document";

export function EncryptedView({ id }: { id: string }) {
  const [run, setRun] = useState<Run>();
  const [error, setError] = useState("");
  const [retry, setRetry] = useState(0);
  const [copy, setCopy] = useState("Copy private link");
  const [contentKey, setContentKey] = useState<string>();
  useEffect(() => {
    const readKey = () => setContentKey(new URLSearchParams(location.hash.slice(1)).get("key") ?? "");
    readKey();
    window.addEventListener("hashchange", readKey);
    return () => window.removeEventListener("hashchange", readKey);
  }, []);
  useEffect(() => {
    if (contentKey === undefined) return;
    const abort = new AbortController();
    setRun(undefined); setError("");
    (async () => {
      const key = contentKey;
      if (!key) throw new Error("This link is missing its encryption key. Ask the sender for the complete private link.");
      const response = await fetch(`/api/shares/${encodeURIComponent(id)}`, { signal: abort.signal, credentials: "omit", cache: "no-store" });
      if (!response.ok) throw new Error(response.status === 404 || response.status === 410 ? "This share is unavailable. It may have been deleted." : "The share could not be downloaded. Please try again.");
      const doc = await openDocument(await readCiphertext(response), key);
      if (!abort.signal.aborted) setRun(doc);
    })().catch(e => { if (!abort.signal.aborted) setError(e instanceof Error ? e.message : "Could not open this share."); });
    return () => abort.abort();
  }, [id, retry, contentKey]);
  if (error) return <section className="encrypted-status" role="alert"><h1>Could not open this share</h1><p>{error}</p><button onClick={() => setRetry(retry + 1)}>Try again</button></section>;
  if (!run) return <p role="status">Opening encrypted share…</p>;
  const download = () => {
    const url = URL.createObjectURL(new Blob([JSON.stringify(run)], { type: "application/json" }));
    const a = document.createElement("a"); a.href = url; a.download = `session-${id}.json`; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
  };
  return <><div className="encrypted-actions"><span>Encrypted share · Anyone with this complete link can read it</span><button onClick={async () => { try { await navigator.clipboard.writeText(location.href); setCopy("Copied"); } catch { setCopy("Could not copy"); } }}>{copy}</button><button onClick={download}>Download document</button></div><RunViewer run={run} /></>;
}
