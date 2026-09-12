// Entry for the standalone viewer bundle (scripts/build-viewer.mjs).
// The local `slink open` server embeds a session as window.__RUN__ and this
// mounts the exact component the hosted site renders — what you preview
// locally is what the recipient sees.
import { useEffect, useState } from "react";
import { createRoot } from "react-dom/client";
import { RunViewer } from "./RunViewer";
import { ShareComposer } from "./ShareComposer";
import type { Run } from "@session-link/format";
import type { LocalViewer } from "./SessionView";

declare global {
  interface Window {
    __RUN__: Run;
    __COMPOSE__?: { source: string };
    __LOCAL__?: LocalViewer;
    // Set when the page carries the reading copy: recorded tool output waits
    // at this address instead of holding up the first screen.
    __FULL__?: string;
  }
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
if (el) createRoot(el).render(window.__COMPOSE__ ? <ShareComposer source={window.__COMPOSE__.source} /> : <LocalSession reading={window.__RUN__} full={window.__FULL__} local={window.__LOCAL__} />);
