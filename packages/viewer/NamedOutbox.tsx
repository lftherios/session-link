import { useEffect, useState } from "react";
import { localRequest } from "./LocalLogin";
import { PrivateSharingStyle } from "./PrivateSharingStyle";
export type NamedRecipient = { id: string; email: string; url: string; status: string };
export type NamedResult = { id: string; url: string; recipients: NamedRecipient[]; error?: string };
const statusText: Record<string,string> = { waiting: "Waiting for recipient", accepted: "Recipient accepted · granting access", ready: "Access granted", revoked: "Access revoked", expired: "Invitation expired" };
export function RecipientLinks({ recipients }: { recipients: NamedRecipient[] }) {
 const [copied, setCopied] = useState("");
 return <ul style={{ paddingLeft: 20, overflowWrap: "anywhere" }}>{recipients.map(r => <li key={r.id} style={{ margin: "16px 0" }}>
  <strong>{r.email}</strong> · {statusText[r.status] ?? r.status}
  {!["revoked", "expired"].includes(r.status) && <div><a href={r.url} target="_blank" rel="noopener noreferrer">{r.status === "ready" ? "Open share" : "Invitation link"}</a> <button onClick={async () => { try { await navigator.clipboard.writeText(r.url); setCopied(r.id); } catch { setCopied("failed"); } }}>{copied === r.id ? "Copied" : "Copy link"}</button></div>}
 </li>)}{copied === "failed" && <li role="alert">Use the invitation link’s context menu to copy its address.</li>}</ul>;
}
export function NamedOutbox() {
 const [shares, setShares] = useState<NamedResult[]>([]), [busy, setBusy] = useState(false), [error, setError] = useState(""), [confirm, setConfirm] = useState("");
 const refresh = async (action = "status", id = "", invite_id = "") => {
  setBusy(true); setError("");
  try { const data = await localRequest<{shares: NamedResult[]}>("/api/named", "POST", { action, id, invite_id }); setShares(data.shares); setConfirm(""); }
  catch (e) { setError((e as Error).message); } finally { setBusy(false); }
 };
 useEffect(() => { void refresh(); }, []);
 return <main className="sl-private" style={{ maxWidth: 720, margin: "24px auto", lineHeight: 1.6, overflowWrap: "anywhere" }}>
  <PrivateSharingStyle />
  <h1>Shared with people</h1>
  <p>Send each person their invitation link. Keep a local viewer running until they accept; access is granted automatically. Unaccepted invitations expire after seven days.</p>
  <button disabled={busy} onClick={() => refresh()}>{busy ? "Checking access…" : "Refresh access"}</button>
  {error && <p role="alert">{error} <a href="/settings">Recovery and devices</a></p>}
  {!busy && !shares.length && <p>No shares with named recipients on this device yet. Open a session, prepare a view, and choose Specific people when publishing.</p>}
  {shares.map(share => <section key={share.id} style={{ borderTop: "1px solid var(--line)", marginTop: 24 }}>
   <h2><a href={share.url} target="_blank" rel="noopener noreferrer">{share.id}</a></h2>
   {share.error && <p role="alert">{share.error}</p>}
   <RecipientLinks recipients={share.recipients} />
   {share.recipients.filter(r => !["revoked", "expired"].includes(r.status)).map(r => <div key={r.id}>
    {confirm === r.id ? <><p>Revoke access for {r.email}? This stops future downloads. Copies and keys already received cannot be erased.</p><button disabled={busy} onClick={() => refresh("revoke", share.id, r.id)}>Confirm revocation</button> <button onClick={() => setConfirm("")}>Cancel</button></> : <button disabled={busy} onClick={() => setConfirm(r.id)}>Revoke {r.email}</button>}
   </div>)}
  </section>)}
 </main>;
}
