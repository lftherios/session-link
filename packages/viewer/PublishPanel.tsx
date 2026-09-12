import { useEffect, useState } from "react";
import { LocalLogin, localRequest } from "./LocalLogin";
import { IdentitySettings } from "./IdentitySettings";
import { RecipientLinks, type NamedRecipient } from "./NamedOutbox";

export function PublishPanel({ endpoint, title, ready = false }: { endpoint: string; title: string; ready?: boolean }) {
 const [open, setOpen] = useState(ready), [signedIn, setSignedIn] = useState<boolean | null>(null);
 const [server, setServer] = useState(""), [busy, setBusy] = useState(false), [error, setError] = useState("");
 const [url, setURL] = useState(""), [copied, setCopied] = useState(false), [backup, setBackup] = useState("");
 const [audience, setAudience] = useState("link"), [emails, setEmails] = useState(""), [keysReady, setKeysReady] = useState(false), [recipients, setRecipients] = useState<NamedRecipient[]>([]);
 useEffect(() => {
  if (audience !== "named" || !signedIn) return;
  localRequest<{state: string; recovery_pending: boolean}>("/api/identity", "POST", {action: "status"})
   .then(data => setKeysReady(data.state === "ready" && !data.recovery_pending)).catch(e => setError(e.message));
 }, [audience, signedIn]);
 useEffect(() => {
  if (!open) return;
  localRequest<{ signed_in?: boolean; state: string; server?: string }>("/api/login/status")
   .then(data => { setSignedIn(!!data.signed_in || data.state === "complete"); setServer(data.server ?? "your server"); })
   .catch(error => setError(error.message));
 }, [open]);
 const publish = async () => {
  setBusy(true); setError("");
  try {
   const selected = emails.split(/[,;\n]/).map(email => email.trim()).filter(Boolean);
   if (audience === "named" && !selected.length) throw new Error("Enter at least one recipient email address.");
   const data = await localRequest<{ url: string; backup?: string; recipients?: NamedRecipient[]; warning?: string }>(endpoint, "POST", audience === "named" ? {recipients: selected} : undefined);
   const shared = new URL(data.url), fragment = new URLSearchParams(shared.hash.slice(1));
   new URLSearchParams(location.hash.slice(1)).forEach((value, name) => { if (["span", "message", "exchange"].includes(name)) fragment.set(name, value); });
   shared.hash = fragment.toString(); setURL(shared.href); setBackup(data.backup ?? ""); setRecipients(data.recipients ?? []);
   if (data.warning) setError(data.warning);
   if (!data.recipients) try { await navigator.clipboard.writeText(shared.href); setCopied(true); } catch { /* The link remains selectable. */ }
  } catch (error) {
   if ((error as { status?: number }).status === 401) { setSignedIn(false); setError("Sign in again to finish publishing this view."); }
   else setError((error as Error).message);
  } finally { setBusy(false); }
 };
 if (!open) return <button className="btn primary" id="pub" onClick={() => setOpen(true)}>Publish link</button>;
 return <section className="sl-publish" aria-label="Publish link" style={{ border: "1px solid var(--line)", borderRadius: 10, padding: 18, margin: "16px 0", overflowWrap: "anywhere" }}>
  {url && recipients.length ? <>
   <p role="status">Published for the people below. Send each person their own invitation link.</p>
   <RecipientLinks recipients={recipients} />
   <p>Keep a local viewer running until they accept. Their access is granted automatically. Unaccepted invitations expire after seven days.</p>
   <a href="/shared" target="_blank" rel="noopener noreferrer">Check or revoke access</a>
  </> : url ? <>
   <p role="status">Published{copied ? " · link copied" : ""}. Anyone with the complete link can view it.</p>
   <p><a href={url} target="_blank" rel="noopener noreferrer">{url}</a></p>
   <button onClick={async () => { try { await navigator.clipboard.writeText(url); setCopied(true); } catch { setError("Select the link above to copy it."); } }}>Copy private link</button>
   <p>{backup === "synced" ? "Share key backed up in your encrypted vault." : backup === "failed" ? "Published, but key backup needs a retry." : "Keep access to your share links on another device."} <a href="/settings" target="_blank" rel="noopener noreferrer">Recovery and devices</a></p>
  </> : signedIn === false ? <LocalLogin onSignedIn={() => { setSignedIn(true); setError(""); }} /> : signedIn === true ? <>
   <p>Publish <strong>{title}</strong> to {server}?</p>
   <label>Who can open this view? <select aria-label="Who can open this view?" value={audience} disabled={busy} onChange={e => { setAudience(e.target.value); setError(""); }}><option value="link">Anyone with the complete link</option><option value="named">Specific people</option></select></label>
   <p>Only the prepared view is encrypted and uploaded. {audience === "named" ? "Recipients sign in with their invited email and unlock an approved browser or device." : "Anyone with the complete link can view it."}</p>
   {audience === "named" && <><label>Recipient emails<textarea aria-label="Recipient emails" value={emails} disabled={busy} onChange={e => setEmails(e.target.value)} placeholder="alex@example.com, sam@example.com" rows={2} style={{display:"block",width:"100%",boxSizing:"border-box",margin:"8px 0",padding:10}} /></label><p>Up to ten people. You’ll copy an invitation link for each person after publishing.</p>{!keysReady && <IdentitySettings onReady={() => setKeysReady(true)} />}</>}
   <button className="sv-primary btn primary" disabled={busy || (audience === "named" && (!keysReady || !emails.trim()))} onClick={publish}>{busy ? "Encrypting and publishing…" : audience === "named" ? "Share with these people" : "Publish encrypted link"}</button>
  </> : <p role="status">Checking sign-in…</p>}
  {error && <p role="alert" className="sv-error">{error}</p>}
 </section>;
}
