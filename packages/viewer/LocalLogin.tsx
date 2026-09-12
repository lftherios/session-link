import { useEffect, useState } from "react";

export async function localRequest<T = Record<string, unknown>>(path: string, method = "GET", body?: unknown): Promise<T> {
 const response = await fetch(path, { method, headers: { "x-slink": "1", ...(body === undefined ? {} : { "content-type": "application/json" }) }, body: body === undefined ? undefined : JSON.stringify(body) });
 const data = await response.json();
 if (!response.ok) throw Object.assign(new Error(data.error?.message ?? `Request failed (${response.status})`), { status: response.status });
 return data;
}

type Attempt = { id: string; state: string; url?: string; user_code?: string; error?: string };
export function LocalLogin({ onSignedIn }: { onSignedIn: () => void }) {
 const [attempt, setAttempt] = useState<Attempt | null>(null), [error, setError] = useState(""), [busy, setBusy] = useState(false);
 useEffect(() => {
  if (attempt?.state !== "pending") return;
  let live = true;
  const timer = setInterval(async () => {
   try {
    const next = await localRequest<Attempt>("/api/login/status");
    if (!live) return;
    if (next.id !== attempt.id) { setAttempt(null); setError("Sign-in was cancelled. Your prepared view is still here."); }
    else if (next.state === "complete") { setAttempt(next); onSignedIn(); }
    else if (next.state === "error") { setAttempt(next); setError(next.error ?? "Sign-in expired. Try again."); }
   } catch { /* A brief connection interruption should not discard the draft. */ }
  }, 1500);
  return () => { live = false; clearInterval(timer); };
 }, [attempt?.id, attempt?.state]);
 const start = async () => {
  // Create the tab in the user gesture. It never receives the local document.
  const popup = window.open("about:blank", "_blank");
  if (popup) popup.opener = null;
  setBusy(true); setError("");
  try { const next = await localRequest<Attempt>("/api/login/start", "POST"); setAttempt(next); if (popup && next.url) popup.location.href = next.url; }
  catch (error) { popup?.close(); setError((error as Error).message); }
  finally { setBusy(false); }
 };
 return <section className="sl-auth" aria-label="Sign in">
  <p>Sign in or create an account with email or GitHub. Your prepared view stays here.</p>
  {attempt?.state === "pending" ? <>
   <p>Enter this code in the sign-in tab: <strong style={{ fontFamily: "var(--mono)", letterSpacing: ".08em" }}>{attempt.user_code}</strong></p>
   <p><a href={attempt.url} target="_blank" rel="noopener noreferrer">Open sign-in</a> · Return here when you’re done.</p>
   <button onClick={async () => { try { await localRequest("/api/login/cancel", "POST"); setAttempt(null); } catch (error) { setError((error as Error).message); } }}>Cancel sign-in</button>
  </> : <button className="sv-primary btn primary" disabled={busy} onClick={start}>{busy ? "Starting…" : "Sign in to continue"}</button>}
  {error && <p role="alert">{error}</p>}
 </section>;
}
