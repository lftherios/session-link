import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { test } from "node:test";
import { buildDocs, siteLink, slug } from "../scripts/build-docs.mjs";

const guide = await readFile("docs/user-guide.md", "utf8");

// The guide is what users are told the CLI does, so it may only name commands
// and flags the CLI has. The command list is the one `slink help` is built from.
test("user guide: every command and flag it names exists in the CLI", async () => {
  const help = await readFile("go/cmd/slink/help.go", "utf8");
  const commands = new Set(help.match(/var commands = \[\]string\{([\s\S]*?)\n\}/)[1].match(/"[a-z]+"/g).map(s => s.slice(1, -1)));
  const sources = (await Promise.all(["commands.go", "devopen.go", "importcmd.go", "main.go", "onoff.go", "statuscmd.go", "viewcmd.go"]
    .map(file => readFile(path.join("go/cmd/slink", file), "utf8")))).join("\n");
  const flags = new Set([...sources.matchAll(/fs\.(?:String|Bool|Int)\("([a-z-]+)"/g)].map(m => m[1]));
  const code = [...guide.matchAll(/```[a-z]*\n([\s\S]*?)```|`([^`\n]+)`/g)].map(m => m[1] ?? m[2]).join("\n");
  const named = [...code.matchAll(/\bslink ([a-z]+)/g)].map(m => m[1]);
  assert.ok(named.length > 20, "the guide shows commands");
  for (const name of named) assert.ok(commands.has(name), `slink ${name} is not a command`);
  // Flags written as the CLI takes them; curl, npm and ssh have their own.
  const ours = code.split("\n").filter(line => !/^(?:curl|npm|brew|ssh) /.test(line.trim())).join("\n");
  for (const [, flag] of ours.matchAll(/(?:^|[\s(])--([a-z][a-z-]+)/g)) assert.ok(flags.has(flag), `--${flag} is not a flag of any command`);
  // The table of commands leaves none out.
  const reference = guide.slice(guide.indexOf("## Command reference"), guide.indexOf("## Files and settings"));
  for (const command of commands) assert.ok(new RegExp("`" + command + "\\b").test(reference), `${command} is missing from the command reference`);
});

test("user guide: links to the repository resolve, there and on the site", () => {
  const links = [...guide.matchAll(/\]\(([^)\s]+)\)/g)].map(m => m[1]).filter(href => !/^https?:|^#/.test(href));
  assert.ok(links.length >= 5);
  for (const href of links) assert.ok(existsSync(path.join("docs", href.split("#")[0])), `${href} does not exist`);
  assert.equal(siteLink("handoff-design.md"), "https://github.com/lftherios/session-link/blob/main/docs/handoff-design.md");
  assert.equal(siteLink("../packages/format"), "https://github.com/lftherios/session-link/tree/main/packages/format");
  assert.equal(siteLink("https://session.link/account"), "https://session.link/account");
  assert.equal(siteLink("#share"), "#share");
});

test("user guide: the page is self-contained and every section can be linked to", async () => {
  const page = await buildDocs();
  assert.match(page, /^<!doctype html>/);
  assert.match(page, /<title>session\.link Docs<\/title>/);
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
