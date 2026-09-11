#!/usr/bin/env node
// Fixture-only preview for viewer work, independent of the Go CLI.
import { createServer } from "node:http";
import { readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { context } from "esbuild";

const root = fileURLToPath(new URL("../", import.meta.url));
const fixtures = new Map([
  ["codex", "testdata/import/codex/basic/golden.run.json"],
  ["claude-code", "testdata/import/claude-code/basic/golden.run.json"],
  ["pi-errors", "testdata/import/pi/error-and-trailing/golden.run.json"],
  ["agent-eval", "packages/format/examples/agent-eval.json"],
  ["chat", "packages/format/examples/chat.json"],
]);
const build = await context({
  entryPoints: [path.join(root, "packages/viewer/viewer-entry.tsx")],
  bundle: true,
  write: false,
  format: "iife",
  jsx: "automatic",
  sourcemap: "inline",
  define: { "process.env.NODE_ENV": '"development"' },
});

const page = `<!doctype html>
<html lang="en"><meta charset="utf-8"><meta name="viewport" content="width=device-width, initial-scale=1">
<title>session.link · viewer preview</title>
<style>
  :root{color-scheme:light dark;background:#f5f6f3;color:#17201c;font-family:system-ui,sans-serif}
  @media(prefers-color-scheme:dark){:root{background:#101512;color:#e3e9e4}}
  :root[data-theme=light]{color-scheme:light;background:#f5f6f3;color:#17201c}
  :root[data-theme=dark]{color-scheme:dark;background:#101512;color:#e3e9e4}
  body{max-width:1160px;margin:0 auto;padding:24px;box-sizing:border-box}
  nav{display:flex;gap:12px;align-items:center;flex-wrap:wrap;margin-bottom:28px;font-size:12px}
  nav span{margin-right:auto}select{font:inherit;padding:5px;border-radius:5px}
  @media(max-width:560px){body{padding:16px}}
</style>
<nav aria-label="Preview controls">
  <span>session.link · viewer preview</span>
  <label>Session <select id="fixture">${[...fixtures.keys()].map((id) => `<option>${id}</option>`).join("")}</select></label>
  <label>Theme <select id="theme"><option>system</option><option>light</option><option>dark</option></select></label>
</nav>
<div id="root">Loading preview…</div>
<script>
  const params = new URLSearchParams(location.search);
  const fixture = document.getElementById('fixture');
  fixture.value = params.get('fixture') || 'codex';
  if (!fixture.value) fixture.value = 'codex';
  fixture.onchange = () => { params.set('fixture', fixture.value); location.search = params.toString(); };
  const theme = document.getElementById('theme');
  theme.value = params.get('theme') || 'system';
  const applyTheme = () => {
    document.documentElement.dataset.theme = theme.value;
    params.set('theme', theme.value);
    history.replaceState(null, '', '?' + params.toString() + location.hash);
  };
  theme.onchange = applyTheme;
  applyTheme();
  fetch('/fixtures/' + fixture.value + '.json').then(r => {
    if (!r.ok) throw new Error('Could not load fixture');
    return r.json();
  }).then(run => {
    window.__RUN__ = run;
    const script = document.createElement('script');
    script.src = '/viewer.js';
    script.onerror = () => { document.getElementById('root').textContent = 'Viewer build failed; check the terminal.'; };
    document.body.append(script);
  }).catch(error => { document.getElementById('root').textContent = error.message; });
</script></html>`;

const server = createServer(async (req, res) => {
  res.setHeader("Cache-Control", "no-store");
  const pathname = new URL(req.url, "http://localhost").pathname;
  try {
    if (pathname === "/") {
      res.setHeader("Content-Type", "text/html; charset=utf-8");
      res.end(page);
    } else if (pathname === "/viewer.js") {
      const result = await build.rebuild(); // Refresh the browser to see edits.
      res.setHeader("Content-Type", "text/javascript; charset=utf-8");
      res.end(result.outputFiles[0].contents);
    } else {
      const id = pathname.match(/^\/fixtures\/([\w-]+)\.json$/)?.[1];
      const fixture = fixtures.get(id);
      if (!fixture) {
        res.writeHead(404).end("Not found");
        return;
      }
      res.setHeader("Content-Type", "application/json; charset=utf-8");
      res.end(await readFile(path.join(root, fixture)));
    }
  } catch (error) {
    console.error(error);
    res.writeHead(500).end("Preview failed; check the terminal.");
  }
});
server.listen(Number(process.env.PORT ?? 4173), "127.0.0.1", () => {
  console.log(`Viewer preview: http://127.0.0.1:${server.address().port}`);
  console.log("Choose a fixture and theme. Refresh after editing viewer code.");
});
server.on("error", async (error) => {
  console.error(error.message);
  await build.dispose();
  process.exitCode = 1;
});
for (const signal of ["SIGINT", "SIGTERM"]) {
  process.once(signal, async () => {
    server.close();
    server.closeAllConnections();
    await build.dispose();
  });
}
