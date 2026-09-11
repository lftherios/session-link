import assert from "node:assert/strict";
import { readFile } from "node:fs/promises";
import { createRequire } from "node:module";
import { test } from "node:test";
import { build } from "esbuild";
import { createElement } from "react";
import { renderToStaticMarkup } from "react-dom/server";

// Exercise the public React component with the same TSX compiler as the
// standalone build. No generated test bundle or extra runtime is needed.
const built = await build({
  entryPoints: ["packages/viewer/index.ts"],
  bundle: true,
  write: false,
  platform: "node",
  format: "cjs",
  external: ["react", "react/*", "react-dom/*", "lucide-react", "@session-link/format"],
  jsx: "automatic",
});
const module = { exports: {} };
new Function("require", "module", "exports", built.outputFiles[0].text)(
  createRequire(import.meta.url), module, module.exports,
);
const { RunViewer } = module.exports;
let modelExports;
const sessionModel = async () => modelExports ??= await (async () => {
  const bundled = await build({ entryPoints: ["packages/viewer/session-model.ts"], bundle: true, write: false, platform: "node", format: "cjs", external: ["@session-link/format"] });
  const model = { exports: {} };
  new Function("require", "module", "exports", bundled.outputFiles[0].text)(createRequire(import.meta.url), model, model.exports);
  return model.exports;
})();

const message = (role, text) => ({ role, content: [{ type: "text", text }] });
const call = (id, input, output, extra = {}) => ({
  id, parent_id: "root", type: "llm_call", model: { id: "test-model" },
  input: { messages: input }, output: { messages: output }, ...extra,
});
const run = (spans, source = { kind: "proxy", fidelity: "exact" }) => ({
  schema: "session/v0", created_at: "2026-09-11T00:00:00Z", source,
  spans: [{ id: "root", type: "agent" }, ...spans],
});
const render = (doc, initialView = "transcript", local) => renderToStaticMarkup(createElement(RunViewer, { run: doc, initialView, local }));
const count = (html, text) => html.split(text).length - 1;

test("arrival: latest input and answer open beneath scoped search and one share action", () => {
  const doc = { ...run([
    call("s1", [message("user", "old-question")], [message("assistant", "old-answer")]),
    call("s2", [message("user", "current-question")], [message("assistant", "current-answer")]),
  ], { kind: "import" }), name: "A recognizable task" };
  const html = render(doc, "exchange", { source: "fixture", project: "/tmp/demo" });
  assert.match(html, /title="Conversation outline"><strong>2<\/strong> <span class="sv-of">of (?:<!-- -->)?2<\/span>/);
  assert.match(html, />Human input</); assert.match(html, />Agent response</);
  assert.doesNotMatch(html, />Prompt<|>Response</);
  assert.doesNotMatch(html.replace(/<style>[\s\S]*?<\/style>/g, ""), />[^<]*\bexchange|aria-label="[^"]*exchange/i, "the reader never shows the internal word exchange");
  assert.ok(html.indexOf("current-question") < html.indexOf("current-answer"));
  assert.doesNotMatch(html, /old-answer|old-question/);
  assert.match(html, /Share this view/);
  assert.match(html, /aria-label="Search scope"/);
  assert.match(html, /Search this view/);
  assert.match(html, /Whole session/);
  assert.match(html, /aria-label="Select human input"/);
  assert.match(html, /aria-label="Select agent response"/);
  assert.match(html, /aria-label="Reading layout"><button aria-pressed="true">Focused<\/button><button aria-pressed="false">Full session<\/button>/);
  assert.match(html, /aria-label="Previous in conversation"/);
  assert.doesNotMatch(html, /class="sv-raw-button"/, "raw session data is offered in the full session layout");
  assert.doesNotMatch(html, /Share this response|Select a passage|aria-label="Selection actions"/);
  assert.match(html, /Edit title/);
  assert.match(html, /aria-label="All sessions"/);
  assert.doesNotMatch(html, /sv-eyebrow|Session actions|local preview/i);
  assert.ok(html.indexOf("<h1>") < html.indexOf("Share this view"), "the title leads and the primary action ends the header row");
  assert.ok(html.indexOf("Share this view") < html.indexOf("Search this view"));
  assert.doesNotMatch(html, /autofocus/i);
  assert.ok(html.indexOf("current-answer") < html.indexOf("Session details"));
  assert.ok(html.indexOf("Session details") < html.indexOf("2026-09-11"), "provenance lives under session details, not above the title");
});

test("navigation: human input is listed by its text or contents, and tool results are not listed", async () => {
  const { exchangesFor, promptLabel } = await sessionModel();
  const doc = run([
    call("s1", [message("user", "Run the checks")], [message("assistant", "Running them")]),
    call("s2", [{ role: "user", content: [{ type: "tool_result", tool_call_id: "t1", content: [{ type: "text", text: "ok" }] }] }], [message("assistant", "All green")]),
    call("s3", [{ role: "user", content: [{ type: "image", media_type: "image/png", data: "" }] }], [message("assistant", "A chart")]),
  ]);
  assert.deepEqual(exchangesFor(doc).map(promptLabel), ["Run the checks", "Image"]);
});

test("navigation: outline previews drop Markdown syntax", async () => {
  const { previewText } = await sessionModel();
  assert.equal(previewText("## Plan\n- **Left, a pager.** Uses ```js\ncode``` and [docs](https://example.test)", 200), "Plan Left, a pager. Uses code and docs");
  assert.equal(previewText("| Before | After |\n| --- | --- |\n| 111 | 13 |", 200), "Before After 111 13");
});

test("arrival: tool results and harness messages never become human input", async () => {
  const { exchangesFor, responseFor, messageText } = await sessionModel();
  const toolResult = (id, text) => ({ role: "user", content: [{ type: "tool_result", tool_call_id: id, content: [{ type: "text", text }] }] });
  const doc = run([
    call("s1", [message("user", "Fix the header layout")], [{ role: "assistant", content: [{ type: "text", text: "Reading the header first." }, { type: "tool_call", id: "t1", name: "Read", arguments: { file_path: "header.tsx" } }] }]),
    call("s2", [toolResult("t1", "export function Header() {}")], [message("assistant", "The header now uses one row.")]),
    call("s3", [message("user", "<local-command-caveat>Caveat: generated by local commands</local-command-caveat>"), message("user", "<command-name>/model</command-name>"), message("user", "<local-command-stdout>Set model</local-command-stdout>"), message("user", "status?")], [message("assistant", "All checks pass.")]),
  ], { kind: "import", harness: "claude-code" });
  const exchanges = exchangesFor(doc);
  assert.deepEqual(exchanges.map(e => e.prompts.map(p => messageText(p.msg))), [["Fix the header layout"], ["status?"]]);
  assert.equal(messageText(responseFor(exchanges[0]).msg), "The header now uses one row.");
  assert.deepEqual(exchanges[1].blocks.map(b => messageText(b.msg).slice(0, 14)), ["<local-command", "<command-name>", "<local-command", "All checks pas"], "context reads with the input it accompanied");
  const html = render(doc, "exchange", { source: "fixture" });
  assert.equal(count(html, ">Human input<"), 1);
  assert.match(html, /status\?/); assert.match(html, /All checks pass/);
  assert.doesNotMatch(html, /local-command|command-name/);
});

test("arrival: imported Claude Code harness messages read as context and errors", async () => {
  const { exchangesFor, responseFor, messageText, readingRole } = await sessionModel();
  const doc = JSON.parse(await readFile("testdata/import/claude-code/harness-messages/golden.run.json", "utf8"));
  const exchanges = exchangesFor(doc);
  assert.deepEqual(exchanges.map(e => e.prompts.map(p => messageText(p.msg))), [["Fix the header layout"], ["status?"]]);
  assert.equal(messageText(responseFor(exchanges[0]).msg), "The header now uses one row.");
  assert.ok(exchanges[0].blocks.some(b => readingRole(b.msg) === "context" && /declined/.test(messageText(b.msg))));
  assert.equal(responseFor(exchanges[1]), undefined);
  const html = render(doc, "exchange", { source: "fixture" });
  assert.match(html, /Error: Usage limit reached\. Try again later\./);
  assert.doesNotMatch(html, />Agent response</);
});

test("arrival: names that look like injected context never become the title", () => {
  const doc = { ...run([call("s1", [message("user", "<environment_context>cwd: /tmp</environment_context>"), message("user", "Compare onboarding options")], [message("assistant", "The comparison")])], { kind: "import" }), name: "<environment_context> <cwd>/tmp</cwd>" };
  const html = render(doc, "exchange");
  assert.match(html, /sv-untitled">Untitled · Import · /);
  assert.doesNotMatch(html, /<h1>&lt;environment_context/);
});

test("arrival: mixed reasoning and answer text use one collapsed activity section", () => {
  const doc = run([call("s1", [message("user", "Compare the options")], [{ role: "assistant", content: [
    { type: "thinking", text: "first-reasoning-marker" },
    { type: "thinking", text: "second-reasoning-marker" },
    { type: "tool_call", id: "lookup", name: "lookup", arguments: { query: "tool-evidence-marker" } },
    { type: "text", text: "The readable answer" },
    { type: "thinking", text: "last-reasoning-marker" },
  ] }])], { kind: "import" });
  const before = JSON.stringify(doc);
  const html = render(doc, "exchange", { source: "fixture" });
  assert.match(html, /The readable answer/);
  assert.equal(count(html, '<summary>Agent activity'), 1);
  assert.doesNotMatch(html, /reasoning-marker|tool-evidence-marker|>thinking</);
  assert.match(html, /id="message-u1-out-0"/);
  assert.equal(JSON.stringify(doc), before, "presentation must not change source content");
  const transcript = render(doc, "transcript");
  assert.match(transcript, /first-reasoning-marker/);
  assert.match(transcript, /last-reasoning-marker/);
});

test("arrival: reasoning without an answer stays in agent activity", () => {
  const doc = run([call("s1", [message("user", "An interrupted question")], [{ role: "assistant", content: [
    { type: "thinking", text: "unfinished-reasoning-marker" },
  ] }])], { kind: "import" });
  const html = render(doc, "exchange");
  assert.match(html, /No response captured/);
  assert.equal(count(html, '<summary>Agent activity'), 1);
  assert.doesNotMatch(html, /unfinished-reasoning-marker|>thinking</);
});

test("arrival: a later child result does not replace the main conversation", () => {
  const doc = run([
    call("s1", [message("user", "main-task")], [message("assistant", "main-result")]),
    { id: "child", parent_id: "s1", type: "agent", name: "Researcher" },
    call("c1", [message("user", "child-task")], [message("assistant", "child-result")], { parent_id: "child" }),
  ]);
  const html = render(doc, "exchange");
  assert.match(html, /main-result/);
  assert.doesNotMatch(html, /child-result|child-task/);
});

test("arrival: trailing prompts and recorded failures never borrow an earlier response", () => {
  const doc = run([
    call("s1", [message("user", "first-task")], [message("assistant", "old-success")]),
    { id: "tail", parent_id: "root", type: "custom", input: { messages: [message("user", "pending-task")] } },
  ]);
  let html = render(doc, "exchange");
  assert.match(html, /pending-task/); assert.match(html, /No response captured/);
  assert.doesNotMatch(html, /old-success|still recording/);
  doc.spans.push(call("failure", [], [], { status: "error", error: { message: "command failed" } }));
  html = render(doc, "exchange", { source: "fixture" });
  assert.match(html, /Error: command failed/);
  assert.match(html, /Share this view/);
});

test("arrival: recovered errors do not override the latest successful exchange", () => {
  const doc = run([
    call("failure", [message("user", "old-task")], [], { status: "error", error: { message: "old-error" } }),
    call("success", [message("user", "new-task")], [message("assistant", "new-result")]),
  ], { kind: "import" });
  const html = render(doc, "exchange");
  assert.match(html, /new-result/); assert.doesNotMatch(html, /old-error/);
});

test("arrival: standalone results remain inspectable and replayed tool results appear once", () => {
  const result = [{ type: "text", text: "recorded-tool-result" }];
  const doc = run([
    call("s1", [message("user", "run tests")], [{ role: "assistant", content: [{ type: "tool_call", id: "t1", name: "shell", arguments: { command: "npm test" } }] }]),
    { id: "t1", type: "tool_call", parent_id: "s1", name: "shell", input: { tool_call_id: "t1", arguments: { command: "npm test" } }, output: { result } },
    call("s2", [{ role: "tool", content: [{ type: "tool_result", tool_call_id: "t1", content: result }] }], [message("assistant", "tests passed")]),
  ]);
  const html = render(doc, "exchange");
  assert.match(html, /Agent activity/);
  assert.equal(count(render(doc, "transcript"), "recorded-tool-result"), 1);
});

test("arrival: opaque names show an untitled label and titles are only editable locally", () => {
  const doc = { ...run([call("s1", [message("user", "Compare onboarding options")], [message("assistant", "The comparison")])]), name: "01a08f55-ced0-7de0-bdf3-b979a0873181" };
  const html = render(doc, "exchange");
  assert.match(html, /<h1 class="sv-untitled">Untitled · Proxy · /);
  assert.doesNotMatch(html, /Edit title|01a08f55/);
  assert.equal(count(html, "Compare onboarding options"), 1, "the prompt is read once, not repeated as a headline");
});

test("arrival: a name the importer clipped from the first prompt is not a title", () => {
  const prompt = "Please delete the stale build artifacts directory and then double-check that nothing else references it";
  const doc = { ...run([call("s1", [message("user", prompt)], [message("assistant", "Done")])], { kind: "import", harness: "claude-code" }), name: prompt.slice(0, 77) + "…" };
  assert.match(render(doc, "exchange"), /<h1 class="sv-untitled">Untitled · Claude Code · /);
  const exact = { ...doc, name: "List the files here" };
  exact.spans = [exact.spans[0], call("s1", [message("user", "List the files here")], [message("assistant", "Done")])];
  assert.match(render(exact, "exchange"), /sv-untitled/);
  const summary = { ...doc, name: "Clean up stale build artifacts" };
  assert.match(render(summary, "exchange"), /<h1>Clean up stale build artifacts<\/h1>/);
});

test("reading: Markdown headings, tables, lists, citations and inert HTML", () => {
  const text = '## Findings\n\n| Product | Setup |\n| --- | --- |\n| Atlas | **Self serve** |\n\n- Verify the trial\n- Read [docs][source]\n\n[source]: https://example.test/docs\n\n<script>alert(1)</script>\n\n[unsafe](javascript:alert(1))';
  const html = render(run([call("s1", [message("user", "Compare products")], [message("assistant", text)])]), "exchange");
  assert.match(html, /<h2>Findings<\/h2>/); assert.match(html, /<table>/); assert.match(html, /<ul>/);
  assert.match(html, /<strong>Self serve<\/strong>/); assert.match(html, /href="https:\/\/example.test\/docs"/);
  assert.match(html, /&lt;script&gt;/); assert.doesNotMatch(html, /<script>|href="javascript:/);
});

test("viewer: an excerpt opens with the author note and the selected outcome", async () => {
  const doc = JSON.parse(await readFile("testdata/share/research/excerpt.json", "utf8"));
  const html = render(doc);
  assert.match(html, /Author note/);
  assert.doesNotMatch(html, /sh-meta">(?:Start here · )?(?:user|assistant)</, "excerpt cards use content model labels, not raw roles");
  assert.match(html, /Start here/);
  assert.ok(html.indexOf("Atlas documents a self-serve") < html.indexOf("Compare Atlas and Beacon"), "outcome should precede supporting prompt");
  assert.match(html, /href="https:\/\/atlas\.example\.test\/onboarding"/);
  assert.match(html, /Inspect included data/);
  assert.doesNotMatch(html, /OMITTED_INTERNAL_STRATEGY/);
  assert.doesNotMatch(html, />exact capture</);
  assert.match(html, /original session was captured exactly/);
});

test("viewer: excerpt omissions are visible and author text remains inert", async () => {
  const doc = JSON.parse(await readFile("testdata/share/research/excerpt.json", "utf8"));
  doc.extensions["session_link.share.v1"].items[1].prompt_missing = true;
  doc.extensions["session_link.share.v1"].references_unavailable = true;
  doc.spans[1].input.messages[0].content[0].text = '<img src=x onerror="alert(1)">';
  const html = render(doc);
  assert.match(html, /Human input not included/);
  assert.match(html, /source references were unavailable/);
  assert.match(html, /&lt;img/);
  assert.doesNotMatch(html, /<img src=x/);
});

test("reading: separately selected source definitions resolve excerpt citations", async () => {
  const doc = JSON.parse(await readFile("testdata/share/research/excerpt.json", "utf8"));
  const share = doc.extensions["session_link.share.v1"];
  const primary = doc.spans.find(span => span.id === share.primary_id);
  primary.input.messages[0].content[0].text = "Finding from [the documentation][docs].";
  doc.spans.push({ id: "source-ref", parent_id: "root", type: "custom", input: { messages: [message("assistant", "[docs]: https://example.test/evidence")] } });
  share.items.push({ id: "source-ref", kind: "source_reference", role: "assistant" });
  const html = render(doc);
  assert.match(html, /href="https:\/\/example.test\/evidence"[^>]*>the documentation<\/a>/);
  assert.match(html, /Source reference/);
  assert.match(html, /\[docs\]:/);
});

test("viewer: full-history calls render each conversational message once", () => {
  const q = message("user", "question-unique");
  const a = message("assistant", "answer-unique");
  const next = message("user", "followup-unique");
  const html = render(run([
    call("s1", [q], [a]),
    call("s2", structuredClone([q, a, next]), [message("assistant", "done-unique")]),
  ]));
  for (const text of ["question-unique", "answer-unique", "followup-unique", "done-unique"]) {
    assert.equal(count(html, text), 1, text);
  }
});

test("viewer: equal-length messages sharing 600+ characters keep their different endings", () => {
  const prefix = "x".repeat(650);
  const html = render(run([
    call("s1", [message("user", prefix + "ending-A")], [message("assistant", "one")]),
    call("s2", [message("user", prefix + "ending-B")], [message("assistant", "two")]),
  ]));
  assert.match(html, /ending-A/);
  assert.match(html, /ending-B/);
});

for (const source of [
  { kind: "import", fidelity: "reconstructed" },
  { kind: "sdk", label: "pi-extension@0.1.0", fidelity: "exact" },
]) {
  test(`viewer: repeated ${source.kind} input deltas remain visible`, () => {
    const prompt = message("user", "continue-unique");
    const html = render(run([
      call("s1", [prompt], []),
      call("s2", [structuredClone(prompt)], [message("assistant", "done")]),
    ], source));
    assert.equal(count(html, "continue-unique"), 2);
  });
}

test("viewer: a shorter input that matches only part of history is a new turn", () => {
  const prompt = message("user", "repeat-unique");
  const html = render(run([
    call("s1", [prompt], [message("assistant", "one")]),
    call("s2", [prompt], [message("assistant", "two")]),
  ]));
  assert.equal(count(html, "repeat-unique"), 2);
});

test("viewer: identical content from a different named speaker is not an echo", () => {
  const prompt = message("user", "speaker-message-unique");
  const html = render(run([
    call("s1", [{ ...prompt, name: "alice" }], []),
    call("s2", [{ ...prompt, name: "bob" }], []),
  ]));
  assert.equal(count(html, "speaker-message-unique"), 2);
});

test("viewer: a parent conversation resumes after an interleaved subagent", () => {
  const q = message("user", "parent-question-unique");
  const a = message("assistant", "parent-answer-unique");
  const html = render(run([
    call("s1", [q], [a]),
    { id: "child", parent_id: "s1", type: "agent", name: "Researcher" },
    call("s2", [q], [message("assistant", "child-answer-unique")], { parent_id: "child" }),
    call("s3", [q, a, message("user", "followup-unique")], [message("assistant", "done")]),
  ]));
  assert.equal(count(html, "parent-question-unique"), 2); // Once in each conversation.
  assert.equal(count(html, "parent-answer-unique"), 1);
  assert.match(html, /child-answer-unique/);
  assert.match(html, /aria-label="Inspect Researcher in the tree"/);
});

test("viewer: a failed retry still shows its error when its input is replayed", () => {
  const prompt = message("user", "retry-question-unique");
  const html = render(run([
    call("s1", [prompt], [], { status: "error", error: { message: "first-failure-unique" } }),
    call("s2", [prompt], [], { status: "error", error: { message: "retry-failure-unique" } }),
  ]));
  assert.equal(count(html, "retry-question-unique"), 1);
  assert.match(html, /first-failure-unique/);
  assert.match(html, /retry-failure-unique/);
});

test("viewer: transcript messages have explicit inspect buttons and escape HTML", () => {
  const html = render(run([
    call("s1", [message("user", '<script>alert("hello")</script>')], []),
    { id: "tail", type: "custom", input: { messages: [message("user", "trailing-unique")] } },
  ]));
  assert.match(html, /<button[^>]*aria-label="Inspect this turn in the tree"/);
  assert.match(html, /&lt;script&gt;/);
  assert.doesNotMatch(html, /<script>/);
  assert.match(html, /trailing-unique/);
});

for (const fixture of [
  "packages/format/examples/agent-eval.json",
  "packages/format/examples/chat.json",
  "testdata/import/codex/basic/golden.run.json",
  "testdata/import/claude-code/basic/golden.run.json",
  "testdata/import/pi/error-and-trailing/golden.run.json",
]) {
  test(`viewer: renders ${fixture}`, async () => {
    const doc = JSON.parse(await readFile(fixture, "utf8"));
    assert.match(render(doc), /aria-label="Viewer mode"/);
  });
}
