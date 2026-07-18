import http from "node:http";
import { spawn } from "node:child_process";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { CAPTURE_DIR, assembleSpool, listCaptures } from "./store.mjs";
import { inspectRunFile, resolveTarget, uploadRun } from "./publish.mjs";

/**
 * `slink open` — browse local captures in the exact viewer the hosted
 * site renders, and publish from the page you're looking at. WYSIWYG
 * egress: zero rendering surprise between preview and the shared URL.
 * Binds to 127.0.0.1 only; publishing requires a custom header and a
 * local Origin so a hostile web page can't drive-by-publish a capture
 * via cross-site POST.
 */

const FILE_ID = /^[A-Za-z0-9._-]+$/;

const CSS = `
  :root{--paper:#f5f6f3;--panel:#fdfdfb;--ink:#17201c;--faint:#5b6660;--line:#d8ddd7;--signal:#0e6f5c;--error:#b3402e;
    --serif:"Iowan Old Style","Palatino Linotype",Palatino,Georgia,serif;
    --mono:ui-monospace,"SF Mono",Menlo,Consolas,monospace}
  *{box-sizing:border-box}body{margin:0;background:var(--paper);color:var(--ink);
    font-family:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif}
  .wrap{max-width:1080px;margin:0 auto;padding:28px 24px 64px}
  .eyebrow{font-family:var(--mono);font-size:11px;letter-spacing:.14em;text-transform:uppercase;color:var(--faint)}
  a{color:var(--signal)}
  .top{display:flex;justify-content:space-between;align-items:center;gap:12px;flex-wrap:wrap}
  .pub{display:flex;align-items:center;gap:10px}
  .result-row{display:flex;justify-content:flex-end;margin:6px 0 16px}
  .btn{border:1px solid var(--line);background:var(--panel);border-radius:6px;padding:7px 14px;
    font-family:var(--mono);font-size:12px;cursor:pointer}
  .btn.primary{background:var(--signal);border-color:var(--signal);color:#fff}
  .note{font-family:var(--mono);font-size:12px;color:var(--faint)}
  .result{font-family:var(--mono);font-size:12px}
  .result.err{color:var(--error);white-space:pre-wrap}
  .card{display:block;background:var(--panel);border:1px solid var(--line);border-radius:10px;
    padding:14px 18px;margin-bottom:10px;color:var(--ink);text-decoration:none;
    transition:border-color .15s ease,box-shadow .15s ease,transform .15s ease}
  .card:hover{border-color:#b9c4bb;box-shadow:0 2px 14px rgba(23,32,28,.07);transform:translateY(-1px)}
  .card:hover .t{color:var(--signal)}
  .card .t{font-family:var(--serif);font-size:18px;margin-bottom:3px}
  .card .m{font-family:var(--mono);font-size:12px;color:var(--faint)}
  .btn{transition:opacity .15s ease}
  .btn:hover{opacity:.88}
`;

const FAVICON = `<link rel="icon" href='data:image/svg+xml,<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 100 100"><rect width="100" height="100" rx="22" fill="%230e6f5c"/><circle cx="50" cy="50" r="16" fill="%23fdfdfb"/></svg>'>`;

const esc = (s) =>
  String(s).replace(/&/g, "&amp;").replace(/</g, "&lt;").replace(/"/g, "&quot;");

function page(title, body) {
  return `<!doctype html><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>${esc(title)}</title>${FAVICON}<style>${CSS}</style><div class="wrap">${body}</div>`;
}

async function indexPage() {
  const captures = await listCaptures();
  const cards = captures
    .map((c) => {
      const id = path.basename(c.file, ".json");
      return `<a class="card" href="/r/${id}"><div class="t">${esc(c.name ?? id)}</div>
        <div class="m">${c.spans} spans · ${esc(c.models.join(", ") || "no models")} · ${esc(c.created_at ?? "")}${c.in_progress ? " · recording" : ""}</div></a>`;
    })
    .join("");
  return page(
    "session.link — local captures",
    `<p class="eyebrow">session.link · local captures · nothing here has left your machine</p>
     <h1 style="font-family:var(--serif);font-weight:500">Captured sessions</h1>
     ${cards || `<p class="note">nothing captured yet — try: slink dev -- &lt;command&gt;</p>`}`,
  );
}

async function runPage(id, target) {
  const file = path.join(CAPTURE_DIR, `${id}.json`);
  await assembleSpool(file).catch(() => null); // live session: fresh snapshot
  const run = JSON.parse(await readFile(file, "utf8"));
  // <-escaping keeps attacker-controlled trace text inert inside the tag
  const json = JSON.stringify(run).replace(/</g, "\\u003c");
  return page(
    run.name ?? id,
    `<div class="top">
       <p class="eyebrow" style="margin:0"><a href="/">← captures</a> · local preview</p>
       <div class="pub">
         <span class="note">unlisted → ${esc(target.server)}${target.apiKey ? "" : " · no API key (slink login)"}</span>
         <button class="btn primary" id="pub">Publish</button>
       </div>
     </div>
     <div class="result-row"><span class="result" id="out"></span></div>
     <div id="root"></div>
     <script>window.__RUN__=${json}</script>
     <script src="/assets/viewer.js"></script>
     <script>
       const btn=document.getElementById("pub"),out=document.getElementById("out");
       btn.onclick=async()=>{
         btn.disabled=true;btn.textContent="Publishing…";out.className="result";out.textContent="";
         const res=await fetch("/api/publish/${id}",{method:"POST",headers:{"x-slink":"1"}});
         const d=await res.json();
         if(d.url){
           const url=d.url+(location.hash||"");
           try{await navigator.clipboard.writeText(url)}catch{}
           out.innerHTML='<a href="'+url+'" target="_blank" rel="noopener"></a>';
           out.firstChild.textContent=url;
           out.append(d.deduplicated?"  (already published, copied)":"  (copied)");
           btn.textContent="Published";
         }else{
           out.className="result err";
           out.textContent="✗ "+(d.error?.message??"failed")+(d.error?.details?"\\n"+d.error.details.map(h=>"  "+(h.pattern??h)+"  "+(h.preview??"")).join("\\n"):"");
           btn.disabled=false;btn.textContent="Publish";
         }
       };
     </script>`,
  );
}

async function publish(id, target) {
  const file = path.join(CAPTURE_DIR, `${id}.json`);
  await assembleSpool(file).catch(() => null); // publish the freshest snapshot
  const info = await inspectRunFile(file);
  if (info.failure) return { status: 400, body: { error: { message: info.failure } } };
  if (info.errors.length)
    return { status: 422, body: { error: { message: "not a valid session/v0 document", details: info.errors } } };
  if (info.secrets.length)
    return {
      status: 422,
      body: { error: { message: "publish blocked — credentials detected; redact the local file first", details: info.secrets } },
    };
  const { ok, status, out } = await uploadRun(info.text, target);
  if (!ok) return { status: status || 502, body: out ?? { error: { message: "upload failed" } } };
  return { status: 200, body: { url: out.url, deduplicated: out.deduplicated } };
}

export async function open(flags) {
  // Load the viewer string only here — `open` is the only command that needs
  // it, so `help`/`list`/`push`/`dev` never require the (gitignored, built)
  // viewer.mjs. Bundlers inline this; from a fresh source tree it's a clear
  // error, not a module-load crash on every command.
  let VIEWER_JS;
  try {
    ({ default: VIEWER_JS } = await import("./viewer.mjs"));
  } catch {
    console.error("error: viewer bundle not built — run `npm run build:viewer` first");
    process.exit(1);
  }

  const target = await resolveTarget(flags);
  const server = http.createServer(async (req, res) => {
    try {
      const url = new URL(req.url ?? "/", "http://localhost");
      const send = (status, type, body) => {
        res.writeHead(status, { "content-type": type });
        res.end(body);
      };
      const m = url.pathname.match(/^\/(r|api\/publish)\/([^/]+)$/);
      const id = m && FILE_ID.test(m[2]) ? m[2] : null;

      if (url.pathname === "/") return send(200, "text/html", await indexPage());
      if (url.pathname === "/assets/viewer.js")
        return send(200, "text/javascript", VIEWER_JS);
      if (m?.[1] === "r" && id && req.method === "GET")
        return send(200, "text/html", await runPage(id, target));
      if (m?.[1] === "api/publish" && id && req.method === "POST") {
        // CSRF guard: custom header forces a CORS preflight (which we never
        // answer), and the Origin must be this server.
        const origin = req.headers.origin;
        if (req.headers["x-slink"] !== "1" || (origin && !origin.startsWith("http://127.0.0.1:") && !origin.startsWith("http://localhost:")))
          return send(403, "application/json", JSON.stringify({ error: { message: "cross-origin publish blocked" } }));
        const { status, body } = await publish(id, target);
        return send(status, "application/json", JSON.stringify(body));
      }
      send(404, "text/plain", "not found");
    } catch (e) {
      res.writeHead(500, { "content-type": "text/plain" });
      res.end(String(e?.message ?? e));
    }
  });

  const port = flags.port ? Number(flags.port) : 4400;
  await new Promise((r, j) => {
    server.once("error", j);
    server.listen(port, "127.0.0.1", r);
  });
  const addr = `http://127.0.0.1:${server.address().port}`;
  console.error(`session.link local viewer · ${addr} · publishes to ${target.server}`);
  if (!flags["no-browser"]) {
    const opener = process.platform === "darwin" ? "open" : "xdg-open";
    spawn(opener, [addr], { stdio: "ignore" }).on("error", () => {});
  }
}
