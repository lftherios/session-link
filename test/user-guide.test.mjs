import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { buildDocs, describe, guideSources, routeOf, siteLink, slug } from "../scripts/build-docs.mjs";

const guide = await readFile("docs/user-guide.md", "utf8");
// The pages under docs/guides answer one question each and are held to the
// same rules as the user guide.
const guides = await Promise.all((await guideSources()).map(async source => ({ source, text: await readFile(source, "utf8") })));
const pages = [{ source: "docs/user-guide.md", text: guide }, ...guides];

// The guide is what users are told the CLI does, so it may only name commands
// and flags the CLI has. The command list is the one `slink help` is built from.
test("user guide: every command and flag it names exists in the CLI", async () => {
  const help = await readFile("go/cmd/slink/help.go", "utf8");
  const commands = new Set(help.match(/var commands = \[\]string\{([\s\S]*?)\n\}/)[1].match(/"[a-z]+"/g).map(s => s.slice(1, -1)));
  const sources = (await Promise.all(["commands.go", "devopen.go", "importcmd.go", "main.go", "onoff.go", "statuscmd.go", "viewcmd.go"]
    .map(file => readFile(path.join("go/cmd/slink", file), "utf8")))).join("\n");
  const flags = new Set([...sources.matchAll(/fs\.(?:String|Bool|Int)\("([a-z-]+)"/g)].map(m => m[1]));
  for (const { source, text } of pages) {
    const code = [...text.matchAll(/```[a-z]*\n([\s\S]*?)```|`([^`\n]+)`/g)].map(m => m[1] ?? m[2]).join("\n");
    const named = [...code.matchAll(/\bslink ([a-z]+)/g)].map(m => m[1]);
    assert.ok(named.length > (text === guide ? 20 : 0), `${source} shows commands`);
    for (const name of named) assert.ok(commands.has(name), `${source}: slink ${name} is not a command`);
    // Flags written as the CLI takes them; curl, npm and ssh have their own.
    const ours = code.split("\n").filter(line => !/^(?:curl|npm|brew|ssh) /.test(line.trim())).join("\n");
    for (const [, flag] of ours.matchAll(/(?:^|[\s(])--([a-z][a-z-]+)/g)) assert.ok(flags.has(flag), `${source}: --${flag} is not a flag of any command`);
  }
  // The table of commands leaves none out.
  const reference = guide.slice(guide.indexOf("## Command reference"), guide.indexOf("## Files and settings"));
  for (const command of commands) assert.ok(new RegExp("`" + command + "\\b").test(reference), `${command} is missing from the command reference`);
});

test("user guide: links to the repository resolve, there and on the site", async () => {
  for (const { source, text } of pages) {
    const links = [...text.matchAll(/\]\(([^)\s]+)\)/g)].map(m => m[1]).filter(href => !/^https?:|^#/.test(href));
    assert.ok(links.length >= 5, `${source} links into the repository`);
    for (const href of links) {
      const [file, anchor] = href.split("#");
      const target = path.join(path.dirname(source), file);
      assert.ok(existsSync(target), `${source}: ${href} does not exist`);
      // A link to a section of another page names a heading that page has.
      if (anchor && routeOf(path.posix.normalize(target))) {
        const headings = [...(await readFile(target, "utf8")).matchAll(/^###? (.+)$/gm)].map(m => slug(m[1]));
        assert.ok(headings.includes(anchor), `${source}: ${href} names no section of that page`);
      }
    }
  }
  assert.equal(siteLink("handoff-design.md"), "https://github.com/lftherios/session-link/blob/main/docs/handoff-design.md");
  assert.equal(siteLink("../packages/format"), "https://github.com/lftherios/session-link/tree/main/packages/format");
  assert.equal(siteLink("https://session.link/account"), "https://session.link/account");
  assert.equal(siteLink("#share"), "#share");
  // One page links to another on the site, from wherever it sits in docs/.
  assert.equal(siteLink("guides/codex.md"), "/docs/codex");
  assert.equal(siteLink("../user-guide.md#share", "docs/guides/codex.md"), "/docs#share");
  assert.equal(siteLink("compare.md", "docs/guides/codex.md", "https://session.link"), "https://session.link/docs/compare");
  assert.equal(siteLink("../handoff-design.md", "docs/guides/codex.md"), "https://github.com/lftherios/session-link/blob/main/docs/handoff-design.md");
  assert.equal(siteLink("../../packages/pi-extension/README.md", "docs/guides/pi.md"), "https://github.com/lftherios/session-link/blob/main/packages/pi-extension/README.md");
});

test("user guide: the page is self-contained and every section can be linked to", async () => {
  const page = await buildDocs();
  assert.match(page, /^<!doctype html>/);
  assert.match(page, /<title>session\.link docs — [^<]+<\/title>/);
  assert.ok(page.includes('<link rel="canonical" href="https://session.link/docs">'));
  const sections = [...guide.matchAll(/^###? (.+)$/gm)].map(m => slug(m[1]));
  assert.ok(sections.length >= 15);
  for (const id of sections) {
    assert.ok(page.includes(` id="${id}"`), `no heading with id ${id}`);
    assert.ok(page.includes(`href="#${id}"`), `the contents list has no link to ${id}`);
  }
  // Nothing is fetched: no stylesheet, script or image from anywhere.
  assert.doesNotMatch(page, /<link[^>]+rel="stylesheet"|<script[^>]+src=|<img /);
  assert.doesNotMatch(page, /\]\([^)]*\)/, "no Markdown link was left unrendered");
  // Recorded text is escaped, not interpreted: the guide's angle brackets survive as text.
  assert.ok(page.includes("&lt;remote-host&gt;"));
  // A preview outside the site carries no document wrapper and points back at the site.
  const fragment = await buildDocs({ fragment: true, origin: "https://session.link" });
  assert.doesNotMatch(fragment, /<!doctype|<html|<head|<body/i);
  assert.ok(fragment.includes('href="https://session.link/#start"'));
});

test("guides: each is a page of its own that a search result can describe", async () => {
  assert.ok(guides.length >= 5);
  const titles = new Set(), descriptions = new Set();
  for (const { source, text } of guides) {
    const route = routeOf(source);
    assert.match(route, /^\/docs\/[a-z0-9-]+$/, `${source} has an address`);
    const page = await buildDocs({ source });
    const [, title] = page.match(/<title>([^<]+) — session\.link<\/title>/) ?? [];
    assert.ok(title && text.startsWith(`# ${title.replace(/&amp;/g, "&")}\n`), `${source}: the page is titled by its first heading`);
    assert.ok(title.length <= 60, `${source}: the title is short enough to be shown whole`);
    assert.ok(page.includes(`<link rel="canonical" href="https://session.link${route}">`), `${source}: canonical address`);
    const [, description] = page.match(/<meta name="description" content="([^"]+)">/) ?? [];
    assert.ok(description && description.length >= 70 && description.length <= 160, `${source}: a description of 70 to 160 characters, not ${description?.length}`);
    assert.match(description, /[.?]$/, `${source}: the description ends on a sentence`);
    titles.add(title); descriptions.add(description);
    // The same rules as the user guide: nothing fetched, nothing left as Markdown.
    assert.doesNotMatch(page, /<link[^>]+rel="stylesheet"|<script[^>]+src=|<img /);
    assert.doesNotMatch(page, /\]\([^)]*\)/, `${source}: no Markdown link was left unrendered`);
    // It leads back to the user guide and on to every other guide.
    assert.ok(page.includes('href="/docs"'));
    for (const other of guides) if (other !== guides.find(g => g.source === source)) assert.ok(page.includes(`href="${routeOf(other.source)}"`), `${source} does not link to ${other.source}`);
  }
  assert.equal(titles.size, guides.length, "no two guides share a title");
  assert.equal(descriptions.size, guides.length, "no two guides share a description");
  // The user guide links to each of them too.
  const home = await buildDocs();
  for (const { source } of guides) assert.ok(home.includes(`href="${routeOf(source)}"`), `the user guide does not link to ${source}`);
});

test("guides: a description is the opening paragraph, cut at a sentence", () => {
  assert.equal(describe("Run `slink view` to read it.\n\n## Next"), "Run slink view to read it.");
  assert.equal(describe("See the [user guide](../user-guide.md) first.\n\nMore."), "See the user guide first.");
  const long = `${"A sentence of some length that says one thing. ".repeat(3)}And a fourth that runs past the limit of what a result shows.`;
  assert.equal(describe(long), "A sentence of some length that says one thing. ".repeat(3).trim());
  assert.ok(describe("word ".repeat(60)).length <= 160);
});
