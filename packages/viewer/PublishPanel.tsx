import { useEffect, useRef, useState } from "react";
import { LocalLogin, localRequest } from "./LocalLogin";
import { IdentitySettings } from "./IdentitySettings";
import { RecipientLinks, type NamedRecipient } from "./NamedOutbox";

export type Published = { url: string; recipients?: number };
type SecretHit = { pattern?: string; preview?: string };
const SECRET_KINDS: Record<string, string> = { provider_api_key: "an API key", stripe_key: "a Stripe key", github_token: "a GitHub token", github_fine_grained_pat: "a GitHub token", aws_access_key_id: "an AWS access key", slack_token: "a Slack token", google_api_key: "a Google API key", private_key_block: "a private key" };

// Names what the scan found and, when the caller can, which included piece holds it.
function blockedMessage(hits: SecretHit[], locate?: (head: string) => string | undefined) {
  const hit = hits[0] ?? {}, head = (hit.preview ?? "").split("…")[0];
  const where = (head && locate?.(head)) || "This view";
  return `Publishing stopped. ${where} contains what looks like ${SECRET_KINDS[hit.pattern ?? ""] ?? "a credential"}${hit.preview ? ` (${hit.preview})` : ""}${hits.length > 1 ? `, plus ${hits.length - 1} more` : ""}. Remove that piece or select a passage without it, then publish again.`;
}

export function PublishPanel({ endpoint, title, ready = false, onPublished, locate }: { endpoint: string; title: string; ready?: boolean; onPublished?: (published: Published) => void; locate?: (head: string) => string | undefined }) {
 const [open, setOpen] = useState(ready), [signedIn, setSignedIn] = useState<boolean | null>(null), section = useRef<HTMLElement>(null);
 // The next step can arrive after the panel was brought into view; keep it there.
 useEffect(() => { if (section.current?.contains(document.activeElement)) section.current.scrollIntoView({ block: "nearest" }); }, [signedIn]);
 const [server, setServer] = useState(""), [busy, setBusy] = useState(false), [error, setError] = useState("");
 const [url, setURL] = useState(""), [copied, setCopied] = useState(false), [backup, setBackup] = useState("");
 const [audience, setAudience] = useState("link"), [emails, setEmails] = useState(""), [keysReady, setKeysReady] = useState<boolean | null>(null), [recipients, setRecipients] = useState<NamedRecipient[]>([]);
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
   onPublished?.({ url: shared.href, recipients: data.recipients?.length });
   if (data.warning) setError(data.warning);
   if (!data.recipients) try { await navigator.clipboard.writeText(shared.href); setCopied(true); } catch { /* The link remains selectable. */ }
  } catch (error) {
   const failure = error as Error & { status?: number; code?: string; details?: SecretHit[] };
   if (failure.status === 401) { setSignedIn(false); setError("Sign in again to finish publishing this view."); }
   else if (failure.code === "secrets_detected") setError(blockedMessage(failure.details ?? [], locate));
   else setError(failure.message);
  } finally { setBusy(false); }
 };
 if (!open) return <button className="btn primary" id="pub" onClick={() => setOpen(true)}>Publish link</button>;
 return <section ref={section} className="sl-publish" aria-label="Publish link" tabIndex={-1} style={{ border: "1px solid var(--line)", borderRadius: 10, padding: 18, margin: "16px 0", overflowWrap: "anywhere" }}>
  {url && recipients.length ? <>
   <p role="status">Published for the people below. Send each person their own invitation link.</p>
   <RecipientLinks recipients={recipients} />
   <p>After each person accepts, this viewer grants their access. Keep it running, or run <code>slink view</code> again later. Unaccepted invitations expire after seven days.</p>
   <a href="/shared" target="_blank" rel="noopener noreferrer">Check or revoke access</a>
  </> : url ? <>
   <p role="status">Published{copied ? " · link copied" : ""}. Anyone with the complete link can view it.</p>
   <p><a href={url} target="_blank" rel="noopener noreferrer">{url}</a></p>
   <button onClick={async () => { try { await navigator.clipboard.writeText(url); setCopied(true); } catch { setError("Select the link above to copy it."); } }}>Copy link</button>
   <p>{backup === "synced" ? "Share key backed up in your encrypted vault." : backup === "failed" ? "Published, but key backup needs a retry." : "Keep access to your share links on another device."} <a href="/settings" target="_blank" rel="noopener noreferrer">Recovery and devices</a></p>
  </> : <>
   {/* Who can open it is the first decision, before any sign-in. */}
   <label>Who can open this view? <select aria-label="Who can open this view?" value={audience} disabled={busy} onChange={e => { setAudience(e.target.value); setError(""); }}><option value="link">Anyone with the complete link</option><option value="named">Specific people</option></select></label>
   <p>Only the prepared view is encrypted and uploaded. {audience === "named" ? "Each person signs in with the email you invite and opens it in a browser or device they have approved." : "Anyone with the complete link can view it."}</p>
   {audience === "named" && <><label>Recipient emails<textarea aria-label="Recipient emails" value={emails} disabled={busy} onChange={e => setEmails(e.target.value)} placeholder="alex@example.com, sam@example.com" rows={2} style={{display:"block",width:"100%",boxSizing:"border-box",margin:"8px 0",padding:10}} /></label><p>Up to ten people. You’ll copy an invitation link for each person after publishing.</p></>}
   {signedIn === false ? <LocalLogin onSignedIn={() => { setSignedIn(true); setError(""); }} /> : signedIn === true ? <>
    {audience === "named" && keysReady === false && <><p>Sharing with specific people needs your own encryption key on this device. Set it up once:</p><IdentitySettings embedded onReady={() => setKeysReady(true)} /></>}
    <p>Publish <strong>{title}</strong> to {server}?</p>
    <button className="sv-primary btn primary" disabled={busy || (audience === "named" && (!keysReady || !emails.trim()))} onClick={publish}>{busy ? "Encrypting and publishing…" : audience === "named" ? "Share with these people" : "Publish encrypted link"}</button>
   </> : <p role="status">Checking sign-in…</p>}
  </>}
  {error && <p role="alert" className="sv-error">{error}</p>}
 </section>;
}
