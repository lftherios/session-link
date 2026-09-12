import { useEffect, useState } from "react";
import { LocalLogin, localRequest } from "./LocalLogin";

type Device = { id: string; name: string; current: boolean };
type Status = {
 server: string; account: string; state: "setup" | "locked" | "ready"; devices: Device[];
 requests: { id: string; name: string; code: string }[];
 shares: { url: string; sha256: string }[];
 confirmation?: string; recovery_key?: string; recovery_pending: boolean;
};
export function IdentitySettings() {
 const [status, setStatus] = useState<Status | null>(null), [signedIn, setSignedIn] = useState<boolean | null>(null);
 const [busy, setBusy] = useState(false), [error, setError] = useState(""), [notice, setNotice] = useState("");
 const [name, setName] = useState(""), [recovery, setRecovery] = useState(""), [newRecovery, setNewRecovery] = useState("");
 const [codes, setCodes] = useState<Record<string, string>>({}), [revoke, setRevoke] = useState("");
 const act = async (action: string, input: Record<string, string> = {}) => {
  setBusy(true); setError(""); setNotice("");
  try {
   const data = await localRequest<Status>("/api/identity", "POST", { action, ...input });
   setStatus(data); if (data.recovery_key) setNewRecovery(data.recovery_key);
   if (action === "confirm-recovery") setNewRecovery("");
   if (action === "recover") setRecovery("");
   if (action === "sync") setNotice("Your share keys are backed up.");
   setRevoke("");
  } catch (error) {
   setError((error as Error).message);
   if ([401,403].includes((error as {status?:number}).status ?? 0)) { setSignedIn(false); setStatus(null); setNewRecovery(""); }
  }
  finally { setBusy(false); }
 };
 useEffect(() => {
  localRequest<{ signed_in?: boolean; state: string }>("/api/login/status")
   .then(data => { const ready = !!data.signed_in || data.state === "complete"; setSignedIn(ready); if (ready) void act("status"); })
   .catch(error => setError(error.message));
 }, []);
 const download = () => {
  const text = `session.link recovery key\nServer: ${status?.server}\nAccount: ${status?.account}\n\n${newRecovery}\n\nKeep this in a password manager or a safe offline place. Anyone with this key and account access can recover your saved share links.\n`;
  const url = URL.createObjectURL(new Blob([text], { type: "text/plain" })); const a = document.createElement("a"); a.href = url; a.download = "session-link-recovery.txt"; a.click(); setTimeout(() => URL.revokeObjectURL(url), 1000);
 };
 return <main className="sl-settings" style={{ maxWidth: 720, margin: "24px auto", lineHeight: 1.6 }}>
  <style>{`.sl-settings h1,.sl-settings h2{font-family:var(--serif);font-weight:500}.sl-settings section{padding:20px 0;border-bottom:1px solid var(--line)}.sl-settings input,.sl-settings textarea{display:block;width:100%;padding:10px;margin:8px 0;background:var(--panel);color:var(--ink);border:1px solid var(--line);border-radius:6px;font:inherit}.sl-settings button,.sl-auth button{padding:8px 12px;border:1px solid var(--line);border-radius:6px;background:var(--panel);color:var(--ink);cursor:pointer;margin:4px 8px 4px 0}.sl-settings button:disabled{opacity:.5;cursor:wait}.sl-settings .secret{font:14px var(--mono);overflow-wrap:anywhere;padding:14px;background:var(--panel);border:1px solid var(--line)}.sl-settings ul{padding-left:20px}.sl-settings li{margin:14px 0;overflow-wrap:anywhere}.sl-settings label{display:block}`}</style>
  <h1>Recovery and devices</h1>
  <p>Back up your private share links so you can open them on another device. Your session files stay on the devices where you saved them.</p>
  {signedIn === false && <LocalLogin onSignedIn={() => { setSignedIn(true); void act("status"); }} />}
  {error && <p role="alert" style={{ color: "var(--error)" }}>{error}</p>}
  {notice && <p role="status">{notice}</p>}
  {signedIn && !status && <button disabled={busy} onClick={() => act("status")}>{busy ? "Loading…" : "Retry"}</button>}
  {status && <fieldset disabled={busy} style={{ border: 0, padding: 0 }}>
   {status.state === "setup" && <section>
    <h2>Set up recovery</h2>
    <p>Your recovery key unlocks your backed-up share links if you lose every approved device. Email sign-in alone cannot restore them.</p>
    <label>Name this device<input aria-label="Device name" value={name} maxLength={80} placeholder="e.g. My laptop" onChange={e => setName(e.target.value)} /></label>
    <button onClick={() => act("setup", { name })}>Create recovery key</button>
   </section>}
   {status.recovery_pending && <section>
    <h2>Save your recovery key</h2>
    {newRecovery ? <>
     <p>Store this key in a password manager or a safe offline place. The server cannot replace it.</p>
     <p className="secret" data-recovery-key>{newRecovery}</p>
     <button onClick={download}>Download recovery key</button>
     <button onClick={() => act("confirm-recovery")}>I saved my recovery key</button>
    </> : <><p>Recovery setup is waiting for you to save the key.</p><button onClick={() => act("setup")}>Show recovery key</button></>}
   </section>}
   {status.state === "locked" && <>
    <section><h2>Approve this device</h2><p>You’re signed in. Access to older share keys needs an approved device or your recovery key.</p>
     <label>Name this device<input aria-label="Device name" value={name} maxLength={80} placeholder="e.g. My laptop" onChange={e => setName(e.target.value)} /></label>
     {status.confirmation && <p>On an approved device, open Recovery and devices and enter this code: <strong className="secret">{status.confirmation}</strong></p>}
     <button onClick={() => act("request", { name })}>{status.confirmation ? "Get a new code" : "Request device approval"}</button>
     <button onClick={() => act("status")}>Check approval</button>
     <p>Requests expire after ten minutes. Only approve a code shown on a device you control.</p>
    </section>
    <section><h2>Use your recovery key</h2><label>Recovery key<input type="password" autoComplete="off" spellCheck={false} value={recovery} onChange={e => setRecovery(e.target.value)} /></label><button disabled={!recovery.trim()} onClick={() => act("recover", { recovery_key: recovery, name })}>Recover share keys</button></section>
   </>}
   {status.state === "ready" && <>
    <section><h2>Approved devices</h2><ul>{status.devices.map(device => <li key={device.id}>
     {device.name}{device.current ? " · this device" : ""}
     {!device.current && (revoke === device.id ? <div><p>Revoke this device? It will lose access to future key backups. Share links it already knows will still work.</p><button onClick={() => act("revoke", { id: device.id })}>Confirm revocation</button><button onClick={() => setRevoke("")}>Cancel</button></div> : <button onClick={() => setRevoke(device.id)}>Revoke</button>)}
    </li>)}</ul><button onClick={() => act("status")}>Refresh devices</button></section>
    {status.requests.length > 0 && <section><h2>Waiting for approval</h2><p>Enter the code displayed on your new device to grant access to backed-up share links.</p><ul>{status.requests.map(request => <li key={request.id}>{request.name}<label>Code from the new device<input aria-label={`Approval code for ${request.name}`} autoComplete="off" value={codes[request.id] ?? ""} placeholder="XXXX-XXXX-XXXX" onChange={e => setCodes({ ...codes, [request.id]: e.target.value })} /></label><button disabled={!codes[request.id]?.trim()} onClick={() => act("approve", { id: request.id, code: codes[request.id] })}>Approve device</button></li>)}</ul></section>}
    <section><h2>Backed-up share links</h2><p>{status.shares.length} {status.shares.length === 1 ? "link" : "links"} in your encrypted vault. New shares from this device are backed up automatically.</p><button onClick={() => act("sync")}>Back up now</button><ul>{status.shares.map(share => <li key={share.url}><a href={share.url} target="_blank" rel="noopener noreferrer">{new URL(share.url).pathname}</a></li>)}</ul></section>
   </>}
  </fieldset>}
 </main>;
}
