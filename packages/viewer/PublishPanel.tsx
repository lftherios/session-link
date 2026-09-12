import { useEffect, useState } from "react";
import { LocalLogin, localRequest } from "./LocalLogin";

export function PublishPanel({ endpoint, title, ready = false }: { endpoint: string; title: string; ready?: boolean }) {
 const [open, setOpen] = useState(ready), [signedIn, setSignedIn] = useState<boolean | null>(null);
 const [server, setServer] = useState(""), [busy, setBusy] = useState(false), [error, setError] = useState("");
 const [url, setURL] = useState(""), [copied, setCopied] = useState(false), [backup, setBackup] = useState("");
 useEffect(() => {
  if (!open) return;
  localRequest<{ signed_in?: boolean; state: string; server?: string }>("/api/login/status")
   .then(data => { setSignedIn(!!data.signed_in || data.state === "complete"); setServer(data.server ?? "your server"); })
   .catch(error => setError(error.message));
 }, [open]);
 const publish = async () => {
  setBusy(true); setError("");
  try {
   const data = await localRequest<{ url: string; backup?: string }>(endpoint, "POST");
   const shared = new URL(data.url), fragment = new URLSearchParams(shared.hash.slice(1));
   new URLSearchParams(location.hash.slice(1)).forEach((value, name) => { if (["span", "message", "exchange"].includes(name)) fragment.set(name, value); });
   shared.hash = fragment.toString(); setURL(shared.href); setBackup(data.backup ?? "");
   try { await navigator.clipboard.writeText(shared.href); setCopied(true); } catch { /* The link remains selectable. */ }
  } catch (error) {
   if ((error as { status?: number }).status === 401) { setSignedIn(false); setError("Sign in again to finish publishing this view."); }
   else setError((error as Error).message);
  } finally { setBusy(false); }
 };
 if (!open) return <button className="btn primary" id="pub" onClick={() => setOpen(true)}>Publish link</button>;
 return <section className="sl-publish" aria-label="Publish link" style={{ border: "1px solid var(--line)", borderRadius: 10, padding: 18, margin: "16px 0", overflowWrap: "anywhere" }}>
  {url ? <>
   <p role="status">Published{copied ? " · link copied" : ""}. Anyone with the complete link can view it.</p>
   <p><a href={url} target="_blank" rel="noopener noreferrer">{url}</a></p>
   <button onClick={async () => { try { await navigator.clipboard.writeText(url); setCopied(true); } catch { setError("Select the link above to copy it."); } }}>Copy private link</button>
   <p>{backup === "synced" ? "Share key backed up in your encrypted vault." : backup === "failed" ? "Published, but key backup needs a retry." : "Keep access to your share links on another device."} <a href="/settings" target="_blank" rel="noopener noreferrer">Recovery and devices</a></p>
  </> : signedIn === false ? <LocalLogin onSignedIn={() => { setSignedIn(true); setError(""); }} /> : signedIn === true ? <>
   <p>Publish <strong>{title}</strong> to {server}?</p>
   <p>Only the prepared view is encrypted and uploaded. Anyone with the complete link can view it.</p>
   <button className="sv-primary btn primary" disabled={busy} onClick={publish}>{busy ? "Encrypting and publishing…" : "Publish encrypted link"}</button>
  </> : <p role="status">Checking sign-in…</p>}
  {error && <p role="alert" className="sv-error">{error}</p>}
 </section>;
}
