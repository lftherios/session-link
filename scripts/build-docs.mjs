#!/usr/bin/env node
/**
 * Render the site's documentation: docs/user-guide.md as the /docs page and
 * each file in docs/guides as /docs/<its name>. Every page is one static,
 * dependency-free HTML file in the hosted pages' palette.
 *
 *   node scripts/build-docs.mjs ../session-link-server/public/docs.html
 *
 * writes docs.html and, beside it, docs/<name>.html for each guide. The server
 * repo commits those files and serves them. The Markdown is the source; edit
 * it, not the HTML. Three options serve a preview of one page outside the site:
 *
 *   --page <name>       write that guide, and only it, to the path given
 *   --fragment          only the title, style and body content, no document wrapper
 *   --origin <url>      prefix for links to the site itself (default: none, same origin)
 */
import { mkdir, readdir, readFile, writeFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import { createElement as h } from "react";
import { renderToStaticMarkup } from "react-dom/server";
import Markdown from "react-markdown";
import remarkGfm from "remark-gfm";

const root = fileURLToPath(new URL("../", import.meta.url));
const REPO = "https://github.com/lftherios/session-link";
const SITE = "https://session.link";
const USER_GUIDE = "docs/user-guide.md";
const GUIDES = "docs/guides";

const text = node => typeof node === "string" || typeof node === "number" ? String(node)
  : Array.isArray(node) ? node.map(text).join("") : node?.props ? text(node.props.children) : "";
export const slug = value => value.toLowerCase().replace(/[^a-z0-9]+/g, "-").replace(/^-|-$/g, "");
const escape = value => value.replace(/[&<>"]/g, c => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" })[c]);

// Where the site serves a source file: the user guide at /docs and a guide at
// /docs/<its name>. Nothing else in the repository has a page.
export const routeOf = source => source === USER_GUIDE ? "/docs"
  : path.posix.dirname(source) === GUIDES && source.endsWith(".md") ? `/docs/${path.posix.basename(source, ".md")}` : null;

// A link from one page to another stays on the site. A link to any other file
// in the repository works on GitHub and nowhere else, so it points there.
export const siteLink = (href, from = USER_GUIDE, origin = "") => {
  if (!href || /^[a-z][a-z0-9+.-]*:|^[#/]/i.test(href)) return href;
  const [file, anchor] = href.split("#");
  const target = path.posix.normalize(path.posix.join(path.posix.dirname(from), file));
  const hash = anchor ? `#${anchor}` : "";
  const route = routeOf(target);
  return route ? `${origin}${route}${hash}` : `${REPO}/${path.posix.extname(target) ? "blob" : "tree"}/main/${target}${hash}`;
};

// What a search result shows under the title: the page's opening paragraph as
// plain text, ending on a sentence within the 160 characters a result has.
export const describe = body => {
  const opening = body.split(/\n\n/)[0].replace(/\s+/g, " ").replace(/\[([^\]]+)\]\([^)]*\)/g, "$1").replace(/[`*]/g, "").trim();
  if (opening.length <= 160) return opening;
  const end = opening.slice(0, 160).lastIndexOf(". ");
  return end > 0 ? opening.slice(0, end + 1) : `${opening.slice(0, 159).replace(/\s+\S*$/, "")}…`;
};

// The first heading is the page title and the line under it says which
// version the page describes; both sit in the page header, not the body.
const readPage = async source => {
  const [, title, status, body] = (await readFile(path.join(root, source), "utf8")).match(/^# (.+)\n\n(.+)\n\n([\s\S]+)$/) ?? [];
  if (!body) throw new Error(`${source} must open with a title and a status line`);
  return { source, route: routeOf(source), title, status, body };
};

export const guideSources = async () =>
  (await readdir(path.join(root, GUIDES))).filter(name => name.endsWith(".md")).sort().map(name => `${GUIDES}/${name}`);

export async function buildDocs({ fragment = false, origin = "", source = USER_GUIDE } = {}) {
  if (!routeOf(source)) throw new Error(`${source} is not a page of the site`);
  const { route, title, status, body } = await readPage(source);
  const others = (await Promise.all([USER_GUIDE, ...await guideSources()].filter(other => other !== source).map(readPage)));
  const more = "More guides";
  const sections = [...[...body.matchAll(/^(##|###) (.+)$/gm)].map(([, level, name]) => ({ deep: level === "###", name, id: slug(name) })),
    { deep: false, name: more, id: slug(more) }];
  const duplicate = sections.find((s, i) => sections.findIndex(o => o.id === s.id) !== i);
  if (duplicate) throw new Error(`two sections of ${source} are both named “${duplicate.name}”`);

  const heading = tag => ({ children }) => h(tag, { id: slug(text(children)) }, children);
  const content = renderToStaticMarkup(h(Markdown, {
    remarkPlugins: [remarkGfm],
    components: {
      h2: heading("h2"), h3: heading("h3"),
      a: ({ href, children }) => { const to = siteLink(href, source, origin); return h("a", /^https?:/.test(to ?? "") ? { href: to, rel: "noopener" } : { href: to }, children); },
      pre: ({ children }) => h("div", { className: "code" }, h("pre", null, children), h("button", { type: "button", className: "copy" }, "Copy")),
      table: ({ children }) => h("div", { className: "table" }, h("table", null, children)),
    },
  }, body));

  const mark = (await readFile(path.join(root, "assets/brand/mark.svg"), "utf8")).trim();
  const brand = (await readFile(path.join(root, "assets/brand/brand.css"), "utf8")).trim();
  const icon = Buffer.from(await readFile(path.join(root, "assets/brand/favicon.svg"))).toString("base64");
  const contents = sections.map(s => `<li${s.deep ? ' class="deep"' : ""}><a href="#${s.id}">${escape(s.name)}</a></li>`).join("");
  const inlineStatus = renderToStaticMarkup(h(Markdown, { components: { p: ({ children }) => h("span", null, children) } }, status));

  // Every page links to every other, so none is reachable only from a list
  // someone has to remember to update.
  const moreGuides = `<h2 id="${slug(more)}">${more}</h2><ul>${others.map(other => `<li><a href="${origin}${other.route}">${escape(other.title)}</a></li>`).join("")}</ul>`;
  const pageTitle = source === USER_GUIDE ? "session.link docs — read and share coding-agent sessions with slink" : `${title} — session.link`;

  const head = `<title>${escape(pageTitle)}</title>
<style>
/* Layout: a sticky brand bar, then a contents column beside one reading column.
   Colours are the hosted pages' tokens; commands sit on the terminal navy. */
:root{--paper:#f6f8ff;--panel:#fff;--soft:#e9eeff;--ink:#0a1033;--faint:#4c557a;--line:#d5dcf5;--signal:#1f44ff;--on-signal:#fff;--term:#0b1030;--term-ink:#e6eaff;--term-dim:#8f99c9;--term-line:#0b1030;
--serif:"Iowan Old Style","Palatino Linotype",Palatino,"Book Antiqua",Georgia,serif;--sans:system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;--mono:ui-monospace,"SF Mono","Cascadia Code",Menlo,Consolas,monospace}
@media (prefers-color-scheme:dark){:root:not([data-theme="light"]){--paper:#0b1030;--panel:#121a45;--soft:#1a2358;--ink:#e6eaff;--faint:#8f99c9;--line:#283266;--signal:#7d9bff;--on-signal:#0a1033;--term:#070b24;--term-ink:#e6eaff;--term-dim:#8f99c9;--term-line:#283266;color-scheme:dark}}
:root[data-theme="dark"]{--paper:#0b1030;--panel:#121a45;--soft:#1a2358;--ink:#e6eaff;--faint:#8f99c9;--line:#283266;--signal:#7d9bff;--on-signal:#0a1033;--term:#070b24;--term-ink:#e6eaff;--term-dim:#8f99c9;--term-line:#283266;color-scheme:dark}
${brand}
*{box-sizing:border-box}
html{scroll-behavior:smooth;scroll-padding-top:88px;-webkit-text-size-adjust:100%}
body{margin:0;background:var(--paper);color:var(--ink);font:16px/1.65 var(--sans);-webkit-font-smoothing:antialiased}
a{color:var(--signal);text-underline-offset:3px}
a:hover{text-decoration:none}
:focus-visible{outline:2px solid var(--signal);outline-offset:3px;border-radius:4px}
.wrap{width:min(1120px,100%);margin:0 auto;padding-inline:32px}
.bar{position:sticky;top:env(safe-area-inset-top,0px);z-index:2;background:color-mix(in srgb,var(--paper) 88%,transparent);backdrop-filter:blur(10px);border-bottom:1px solid var(--line)}
.bar .wrap{display:flex;align-items:center;justify-content:space-between;gap:16px;min-height:64px}
.brand-row{display:flex;align-items:baseline;gap:14px;min-width:0}
.brand-row .here{font:12px/1 var(--mono);letter-spacing:.1em;text-transform:uppercase;color:var(--faint);text-decoration:none}
.brand-row a.here:hover{color:var(--signal)}
.slink-brand svg{align-self:center}
.links{display:flex;align-items:center;gap:18px;font-size:14px;white-space:nowrap}
.links a{color:var(--ink);text-decoration:none}
.links a:hover{color:var(--signal)}
.links .btn{padding:8px 16px;border-radius:999px;background:var(--signal);color:var(--on-signal);font-weight:600}
.links .btn:hover{color:var(--on-signal);filter:brightness(1.08)}
.page{display:grid;grid-template-columns:220px minmax(0,1fr);gap:56px;align-items:start;padding-block:48px 96px}
.contents{position:sticky;top:calc(env(safe-area-inset-top,0px) + 88px);max-height:calc(100vh - 120px);overflow:auto;font-size:14px}
.contents summary{font:11px/1.5 var(--mono);letter-spacing:.12em;text-transform:uppercase;color:var(--faint);list-style:none;margin-bottom:10px}
.contents summary::-webkit-details-marker{display:none}
.contents ol{display:grid;gap:2px;margin:0;padding:0;list-style:none}
.contents a{display:block;padding:5px 10px;border-left:2px solid var(--line);color:var(--faint);text-decoration:none}
.contents a:hover,.contents a[aria-current]{color:var(--ink);border-left-color:var(--signal)}
.contents .deep a{padding-left:24px;font-size:13px}
article{min-width:0;max-width:72ch}
h1,h2,h3{text-wrap:balance}
h1{margin:0;font:500 clamp(34px,5vw,48px)/1.08 var(--serif);letter-spacing:-.02em}
.status{margin:14px 0 0;font:12px/1.6 var(--mono);color:var(--faint)}
.status code{font-size:inherit;background:none;padding:0}
h2{margin:56px 0 0;padding-top:28px;border-top:1px solid var(--line);font:500 30px/1.15 var(--serif);letter-spacing:-.015em}
h3{margin:36px 0 0;font:600 18px/1.3 var(--sans);letter-spacing:-.01em}
p,ul,ol,.code,.table{margin:16px 0 0}
.status+p{font-size:18px;line-height:1.6}
ul,article ol{padding-left:22px}
li+li{margin-top:6px}
li>ul{margin-top:6px}
code{font:.875em/1.5 var(--mono);background:var(--soft);padding:2px 5px;border-radius:5px;overflow-wrap:anywhere}
.code{position:relative;border:1px solid var(--term-line);border-radius:10px;background:var(--term);overflow:hidden}
.code pre{margin:0;padding:16px 18px;overflow-x:auto;color:var(--term-ink);font:13.5px/1.7 var(--mono)}
.code code{font:inherit;background:none;padding:0;border-radius:0;overflow-wrap:normal;white-space:pre}
.copy{position:absolute;top:8px;right:8px;padding:4px 10px;border:1px solid color-mix(in srgb,var(--term-dim) 45%,transparent);border-radius:6px;background:var(--term);color:var(--term-dim);font:11px/1.5 var(--mono);cursor:pointer}
.copy:hover{color:var(--term-ink)}
.table{overflow-x:auto;border:1px solid var(--line);border-radius:10px;background:var(--panel)}
table{width:100%;border-collapse:collapse;font-size:14.5px;line-height:1.5}
th,td{padding:10px 14px;text-align:left;vertical-align:top;border-bottom:1px solid var(--line)}
th{font:11px/1.5 var(--mono);letter-spacing:.1em;text-transform:uppercase;color:var(--faint);white-space:nowrap}
tr:last-child td{border-bottom:0}
th:first-child,td:first-child{width:32%}
.foot{border-top:1px solid var(--line);padding-block:28px;color:var(--faint);font-size:13px}
.foot .wrap{display:flex;flex-wrap:wrap;gap:8px 24px;justify-content:space-between}
@media(max-width:900px){.page{grid-template-columns:minmax(0,1fr);gap:28px;padding-block:28px 64px}
.contents{position:static;max-height:none;border:1px solid var(--line);border-radius:10px;background:var(--panel);padding:12px 14px}
.contents summary{margin:0;cursor:pointer}.contents[open] summary{margin-bottom:10px}
th:first-child,td:first-child{width:38%}}
@media(max-width:680px){.wrap{padding-inline:20px}.brand-row .here,.links .plain{display:none}h2{font-size:26px}}
@media(prefers-reduced-motion:reduce){html{scroll-behavior:auto}}
</style>`;

  const page = `<div class="bar"><div class="wrap">
  <div class="brand-row"><a class="slink-brand slink-brand--compact" href="${origin}/" aria-label="session.link home">${mark}<span class="slink-brand-name">session<span class="slink-brand-dot">.</span>link</span></a>${source === USER_GUIDE ? '<span class="here">Docs</span>' : `<a class="here" href="${origin}/docs">Docs</a>`}</div>
  <nav class="links" aria-label="Site"><a class="plain" href="${REPO}" rel="noopener">GitHub ↗</a><a class="btn" href="${origin}/#start">Get started</a></nav>
</div></div>
<div class="wrap page">
  <details class="contents" open><summary>On this page</summary><ol>${contents}</ol></details>
  <article>
    <h1>${escape(title)}</h1>
    <p class="status">${inlineStatus}</p>
    ${content}
    ${moreGuides}
  </article>
</div>
<footer class="foot"><div class="wrap"><span>Written from <a href="${REPO}/blob/main/${source}" rel="noopener">${source}</a> in the session-link repository.</span><span><a href="${origin}/">session.link</a></span></div></footer>
<script>
// Copy buttons, and the contents list marking the section being read. The
// page reads the same without either.
for (const button of document.querySelectorAll('.copy')) button.addEventListener('click', async () => {
  const code = button.parentElement.querySelector('pre');
  try { await navigator.clipboard.writeText(code.textContent.trimEnd()); button.textContent = 'Copied'; }
  catch { const range = document.createRange(); range.selectNodeContents(code); const selection = getSelection(); selection.removeAllRanges(); selection.addRange(range); button.textContent = 'Selected'; }
  setTimeout(() => { button.textContent = 'Copy'; }, 1600);
});
if (matchMedia('(max-width: 900px)').matches) document.querySelector('.contents').open = false;
if ('IntersectionObserver' in window) {
  const links = new Map([...document.querySelectorAll('.contents a')].map(a => [a.getAttribute('href').slice(1), a]));
  const seen = new IntersectionObserver(entries => {
    for (const entry of entries) if (entry.isIntersecting) {
      for (const a of links.values()) a.removeAttribute('aria-current');
      links.get(entry.target.id)?.setAttribute('aria-current', 'true');
    }
  }, { rootMargin: '-90px 0px -70% 0px' });
  for (const id of links.keys()) { const el = document.getElementById(id); if (el) seen.observe(el); }
}
</script>`;

  if (fragment) return `${head}\n${page}\n`;
  return `<!doctype html>
<html lang="en">
<head>
<meta charset="utf-8">
<meta name="viewport" content="width=device-width, initial-scale=1">
<meta name="description" content="${escape(describe(body))}">
<link rel="canonical" href="${SITE}${route}">
<meta property="og:title" content="${escape(title)}">
<meta property="og:description" content="${escape(describe(body))}">
<meta property="og:type" content="article">
<meta property="og:url" content="${SITE}${route}">
<link rel="icon" href="data:image/svg+xml;base64,${icon}" type="image/svg+xml">
<!-- Generated from ${source} in the session-link repository by scripts/build-docs.mjs. Edit that file, not this one. -->
${head}
</head>
<body>
${page}
</body>
</html>
`;
}

if (process.argv[1] && fileURLToPath(import.meta.url) === path.resolve(process.argv[1])) {
  const args = process.argv.slice(2);
  const take = name => { const i = args.indexOf(name); return i < 0 ? undefined : args.splice(i, 2)[1]; };
  const origin = take("--origin") ?? "";
  const only = take("--page");
  const fragment = args.includes("--fragment");
  const out = args.find(arg => !arg.startsWith("--"));
  if (!out) {
    console.error("usage: node scripts/build-docs.mjs <output.html> [--page <guide>] [--fragment] [--origin https://session.link]");
    process.exit(1);
  }
  const write = async (file, source) => {
    const html = await buildDocs({ fragment, origin, source });
    await writeFile(file, html);
    console.log(`built ${file} (${(html.length / 1024).toFixed(0)} KB) from ${source}`);
  };
  await write(out, only ? `${GUIDES}/${only}.md` : USER_GUIDE);
  // The guides go in a directory named after the user guide's file: docs.html
  // and docs/<name>.html, which is where the site looks for /docs/<name>.
  if (!only && !fragment) {
    const dir = out.replace(/\.html$/, "");
    await mkdir(dir, { recursive: true });
    const sources = await guideSources();
    for (const source of sources) await write(path.join(dir, `${path.posix.basename(source, ".md")}.html`), source);
    // A page whose source is gone would still be served, so say so. Removing
    // it is left to whoever commits the directory.
    const built = new Set(sources.map(source => `${path.posix.basename(source, ".md")}.html`));
    for (const file of await readdir(dir)) if (file.endsWith(".html") && !built.has(file)) console.warn(`stale: ${path.join(dir, file)} has no source in ${GUIDES}; remove it`);
  }
}
