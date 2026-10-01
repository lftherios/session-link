// Entry for the standalone viewer bundle (scripts/build-viewer.mjs).
// The local `slink open` server embeds a session as window.__RUN__ and this
// mounts the exact component the hosted site renders — what you preview
// locally is what the recipient sees.
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { RunViewer } from "./RunViewer";
import { ShareComposer } from "./ShareComposer";
import { IdentitySettings } from "./IdentitySettings";
import { PublishPanel } from "./PublishPanel";
import { EncryptedView } from "./EncryptedView";
import { NamedView } from "./NamedView";
import { NamedOutbox } from "./NamedOutbox";
import type { Run } from "@session-link/format";
import type { LocalViewer } from "./SessionView";

declare global {
  interface Window {
    __RUN__: Run;
    __IDENTITY__?: boolean;
    __BROWSER_IDENTITY__?: boolean;
    __NAMED__?: { id: string };
    __NAMED_OUTBOX__?: boolean;
    __PUB__?: { endpoint: string; title: string };
    __SEALED__?: { id: string };
    __COMPOSE__?: { source: string };
    __LOCAL__?: LocalViewer;
    // Set when the page carries the reading copy: recorded tool output waits
    // at this address instead of holding up the first screen.
    __FULL__?: string;
  }
}

// A hosted page can hand over the session without any inline script, which
// keeps it compatible with a strict Content-Security-Policy: a small session
// travels in <script type="application/json" id="run-data">, a large one is
// named by data-src on #root and fetched by the viewer. The local viewer keeps
// setting window.__RUN__ directly; that path wins when both are present.
function hostedSession(root: HTMLElement | null): { run?: Run; src?: string } {
  const embedded = document.getElementById("run-data")?.textContent;
  if (embedded) {
    try { return { run: JSON.parse(embedded) as Run }; } catch { /* fall through to data-src */ }
  }
  const src = root?.dataset.src;
  return src ? { src } : {};
}

function LocalSession({ reading, full, local }: { reading: Run; full?: string; local?: LocalViewer }) {
  const [run, setRun] = useState(reading);
  useEffect(() => {
    if (!full) return;
    let alive = true;
    fetch(full)
      .then(response => (response.ok ? (response.json() as Promise<Run>) : Promise.reject(new Error(String(response.status)))))
      .then(whole => { if (alive) setRun(whole); })
      .catch(() => {}); // the reading copy stays; every step but recorded output is there
    return () => { alive = false; };
  }, [full]);
  return <RunViewer run={run} local={local} />;
}

const el = document.getElementById("root");
const hosted = window.__RUN__ ? {} : hostedSession(el);
if (el) createRoot(el).render(window.__NAMED__ ? <NamedView id={window.__NAMED__.id} /> : window.__NAMED_OUTBOX__ ? <NamedOutbox /> : window.__BROWSER_IDENTITY__ ? <IdentitySettings browser /> : window.__IDENTITY__ ? <IdentitySettings /> : window.__SEALED__ ? <EncryptedView id={window.__SEALED__.id} /> : window.__COMPOSE__ ? <ShareComposer source={window.__COMPOSE__.source} /> : hosted.src ? <RunViewer src={hosted.src} /> : <LocalSession reading={hosted.run ?? window.__RUN__} full={window.__FULL__} local={window.__LOCAL__} />);

const publish = document.getElementById("publish-control");
if (publish && window.__PUB__) createRoot(publish).render(<PublishPanel {...window.__PUB__} />);
