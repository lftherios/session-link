"use client";
import { useEffect, useState } from "react";
import type { Run } from "@session-link/format";
import { RunViewer } from "./RunViewer";
import { decryptDocument, MAX_PLAINTEXT, HEADER_SIZE } from "./encryption";
import validate from "./validate-session.js";

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
      const reader = response.body?.getReader();
      if (!reader) throw new Error("The share could not be downloaded.");
      const chunks: Uint8Array[] = []; let size = 0;
      try {
        while (true) {
          const part = await reader.read(); if (part.done) break;
          size += part.value.length;
          if (size > MAX_PLAINTEXT + HEADER_SIZE + 16) throw new Error("This share exceeds the supported size.");
          chunks.push(part.value);
        }
      } finally { await reader.cancel().catch(() => {}); }
      const bytes = new Uint8Array(size); let offset = 0;
      for (const part of chunks) { bytes.set(part, offset); offset += part.length; }
      const plain = await decryptDocument(bytes, key);
      const doc = JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(plain));
      if (!validate(doc)) throw new Error("The decrypted document is not a valid session.");
      const ids = new Map<string, string | undefined>();
      for (const span of doc.spans) { if (ids.has(span.id)) throw new Error("The document contains duplicate steps."); ids.set(span.id, span.parent_id ?? undefined); }
      const checked = new Set<string>();
      for (const id of ids.keys()) {
        const seen = new Set<string>(); let current: string | undefined = id;
        while (current && ids.has(current) && !checked.has(current)) {
          if (seen.has(current)) throw new Error("The document contains a cyclic step tree.");
          seen.add(current); current = ids.get(current);
        }
        for (const item of seen) checked.add(item);
      }
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
