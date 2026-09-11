// Entry for the standalone viewer bundle (scripts/build-viewer.mjs).
// The local `slink open` server embeds a run as window.__RUN__ and this
// mounts the exact component the hosted site renders — what you preview
// locally is what the recipient sees.
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
  }
}

const el = document.getElementById("root");
if (el) createRoot(el).render(window.__COMPOSE__ ? <ShareComposer source={window.__COMPOSE__.source} /> : <RunViewer run={window.__RUN__} local={window.__LOCAL__} />);
