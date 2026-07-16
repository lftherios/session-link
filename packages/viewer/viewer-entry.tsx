// Entry for the standalone viewer bundle (scripts/build-viewer.mjs).
// The local `slink open` server embeds a run as window.__RUN__ and this
// mounts the exact component the hosted site renders — what you preview
// locally is what the recipient sees.
import { createRoot } from "react-dom/client";
import { RunViewer } from "./RunViewer";
import type { Run } from "@session-link/format";

declare global {
  interface Window {
    __RUN__: Run;
  }
}

const el = document.getElementById("root");
if (el) createRoot(el).render(<RunViewer run={window.__RUN__} />);
