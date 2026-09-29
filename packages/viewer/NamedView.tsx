import { useCallback, useEffect, useState } from "react";
import type { Run } from "@session-link/format";
import { RunViewer } from "./RunViewer";
import { IdentitySettings } from "./IdentitySettings";
import { browserIdentity, browserRequest, unlockedIdentity } from "./browser-identity";
import { bindingProof, hash, hexHash, sign, unb64 } from "./identity-crypto";
import { namedContentKey, verifyClaim, verifyInvite, type NamedShare, type RecipientClaim } from "./named-sharing";
import { openDocument, readCiphertext } from "./sealed-document";
import { PrivateSharingStyle } from "./PrivateSharingStyle";

type InviteSecret = { id: string; secret: string; sender: string };
function invitation(id: string): InviteSecret | undefined {
 const query = new URLSearchParams(location.search), fragment = new URLSearchParams(location.hash.slice(1)), key = `slink-invitation/${id}`;
 if (fragment.has("invite")) {
  const value = { id: query.get("invite") ?? "", secret: fragment.get("invite") ?? "", sender: fragment.get("sender") ?? "" };
  unb64(value.id, 24); unb64(value.secret, 32); unb64(value.sender, 32);
  // Tab-scoped storage survives the email/GitHub sign-in redirect. The
  // invitation secret is never included in a request or the login's next URL.
  sessionStorage.setItem(key, JSON.stringify(value));
  return value;
 }
 const saved = sessionStorage.getItem(key);
 return saved ? JSON.parse(saved) as InviteSecret : undefined;
}
export function NamedView({ id }: { id: string }) {
 const [run, setRun] = useState<Run>(), [share, setShare] = useState<NamedShare>();
 const [mode, setMode] = useState("loading"), [error, setError] = useState(""), [busy, setBusy] = useState(false), [retry, setRetry] = useState(0);
 const [copy, setCopy] = useState("Copy share link");
 const ready = useCallback(() => setRetry(n => n + 1), []);
 useEffect(() => {
  const abort = new AbortController(); let timer: ReturnType<typeof setTimeout> | undefined;
  setError("");
  (async () => {
   const invite = invitation(id), selected = new URLSearchParams(location.search).get("invite") ?? invite?.id;
   const data = await browserRequest<NamedShare>(`/api/named-shares/${id}${selected ? "?invite=" + encodeURIComponent(selected) : ""}`);
   if (data.id !== id) throw new Error("The server returned a different share.");
   const invited = await verifyInvite(data);
   if (invite && invite.sender !== invited.root) throw new Error("The sender's encryption identity does not match your invitation.");
   if (abort.signal.aborted) return;
   setShare(data);
   const status = await browserIdentity();
   if (abort.signal.aborted) return;
   if (status.state !== "ready" || status.recovery_pending) { setMode("identity"); return; }
   if (!status.sharing_ready) await browserIdentity("sharing");
   const identity = await unlockedIdentity();
   if (data.owner || data.grant) {
    const key = await namedContentKey(data, identity);
    const response = await fetch(`/api/named-shares/${id}/blob`, { credentials: "same-origin", cache: "no-store", signal: abort.signal });
    if (!response.ok) throw new Error("Access is no longer available. Ask the sender to check this share.");
    const bytes = await readCiphertext(response);
    if (await hexHash(bytes) !== invited.sha256) throw new Error("The downloaded share failed its integrity check.");
    const document = await openDocument(bytes, key);
    if (!abort.signal.aborted) {
     sessionStorage.removeItem(`slink-invitation/${id}`);
     const fragment = new URLSearchParams(location.hash.slice(1)); fragment.delete("invite"); fragment.delete("sender");
     history.replaceState(null, "", `/n/${id}${fragment.size ? "#" + fragment.toString() : ""}`);
     setRun(document); setMode("ready");
    }
   } else if (data.claim) {
    await verifyClaim(data, invited, identity);
    if (!abort.signal.aborted) { setMode("waiting"); timer = setTimeout(ready, 5000); }
   } else {
    if (data.invitation?.status === "expired") throw new Error("This invitation expired. Ask the sender for a new share.");
    if (!invite || invite.id !== data.invitation?.id) throw new Error("Open the complete invitation link sent to your email address to accept this share.");
    if (invited.invitation.id !== invite.id || await hash(unb64(invite.secret, 32)) !== invited.invitation.commitment) throw new Error("This invitation secret does not match the sender's invitation.");
    if (!abort.signal.aborted) setMode("accept");
   }
  })().catch(e => { if (!abort.signal.aborted) { setRun(undefined); setError(e.message); setMode(e.status === 401 ? "login" : e.status === 403 ? "account" : "error"); } });
  return () => { abort.abort(); clearTimeout(timer); };
 }, [id, retry, ready]);
 const accept = async () => {
  setBusy(true); setError("");
  try {
   const invite = invitation(id), identity = await unlockedIdentity();
   if (!share || !invite) throw new Error("Open the complete invitation link again.");
   const invited = await verifyInvite(share);
   if (invited.root !== invite.sender || invited.invitation.id !== invite.id || await hash(unb64(invite.secret, 32)) !== invited.invitation.commitment || share.invitation?.id !== invite.id) throw new Error("This invitation failed verification.");
   const claim: RecipientClaim = { account: identity.account, root: identity.state.root, head: identity.head, signer: identity.deviceID, share_id: id, invite_id: invite.id, sha256: invited.sha256, owner: invited.account, owner_root: invited.root, inbox: identity.state.inbox!, epoch: identity.state.epoch };
   const signed = await sign("recipient-claim", claim, identity.device.sign);
   await browserRequest(`/api/named-shares/${id}`, "POST", { action: "claim", claim: { ...signed, proof: await bindingProof(invite.secret, signed) } });
   ready();
  } catch (e) { setError((e as Error).message); } finally { setBusy(false); }
 };
 if (run) return <><div className="encrypted-actions"><span>Encrypted share · Access limited to invited accounts</span><button onClick={async () => { try { await navigator.clipboard.writeText(location.href); setCopy("Copied"); } catch { setCopy("Could not copy"); } }}>{copy}</button><a href="/devices">Recovery and devices</a></div><RunViewer run={run} /></>;
 const next = location.pathname + location.search;
 return <section className="encrypted-status sl-private" style={{ maxWidth: 720, margin: "24px auto", lineHeight: 1.6, overflowWrap: "anywhere" }}>
  <PrivateSharingStyle />
  <h1>Private session share</h1>
  {error && <p role="alert">{error}</p>}
  {mode === "login" ? <p><a href={`/login?next=${encodeURIComponent(next)}`}>Sign in to open this share</a></p> : mode === "account" ? <p><a href={`/login?next=${encodeURIComponent(next)}&link=1`}>Link the invited email</a> · <a href="/account">Switch account</a></p> : mode === "identity" ? <><p>Set up or unlock this browser to accept encrypted shares.</p><IdentitySettings browser onReady={ready} /></> : mode === "accept" ? <><p>This invitation is for <strong>{share?.invitation?.email}</strong>. Accept it to let the sender grant access to your approved devices.</p><button disabled={busy} onClick={accept}>{busy ? "Accepting…" : "Accept invitation"}</button><p>The sender’s local viewer needs to be running to complete the first grant.</p></> : mode === "waiting" ? <><p role="status">Invitation accepted. Waiting for the sender to grant access…</p><p>This page opens automatically once the sender runs their local viewer.</p></> : mode === "error" ? <button onClick={ready}>Try again</button> : <p role="status">Checking private access…</p>}
 </section>;
}
