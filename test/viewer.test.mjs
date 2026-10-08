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

test("privacy: transcript images never load remote or local HTTP resources", () => {
  const doc = run([call("s1", [message("user", "Review this")], [{ role:"assistant", content:[
    {type:"text",text:"![Diagram](https://external.test/private-marker.png)"},
    {type:"image",url:"http://127.0.0.1:4400/api/stop"},
    {type:"image",url:"javascript:alert(1)"},
    {type:"image",url:"data:image/svg+xml,<svg onload='alert(1)'/>"},
    {type:"image",url:"data:image/png;base64,iVBORw0KGgo="},
  ]}])]);
  const html = render(doc, "exchange");
  assert.doesNotMatch(html, /<img[^>]+src="(?:https?:|javascript:|data:image\/svg)/);
  assert.match(html, /href="https:\/\/external.test\/private-marker.png" target="_blank" rel="noopener noreferrer"/);
  assert.match(html, /Open external image/);
  assert.match(html, /<img[^>]+src="data:image\/png;base64,/);
});

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
  assert.doesNotMatch(html.slice(0, html.indexOf('aria-label="Continue reading"')), /old-answer|old-question/, "the earlier exchange is only named in the navigation after the answer");
  assert.doesNotMatch(html, /old-answer/);
  assert.match(html, /Share this view/);
  assert.match(html, /aria-label="Search scope"/);
  assert.match(html, /Search this view/);
  assert.match(html, /Whole session/);
  assert.doesNotMatch(html, /class="sv-block-tools"|aria-label="(?:Select|Deselect) |aria-label="Content actions"/, "messages carry no selection or action controls");
  assert.match(html, /aria-label="Reading layout"><button aria-pressed="true">Focused<\/button><button aria-pressed="false">Full session<\/button>/);
  assert.match(html, /aria-label="Previous in conversation"/);
  assert.doesNotMatch(html, /class="sv-raw-button"/, "raw session data is offered in the full session layout");
  assert.doesNotMatch(html, /class="sv-time"/, "captures without span times show no times");
  assert.doesNotMatch(html, /Share this response|Select a passage|aria-label="Selection actions"/);
  assert.match(html, /Edit title/);
  assert.match(html, /aria-label="All sessions"/);
  assert.doesNotMatch(html, /sv-eyebrow|Session actions|local preview/i);
  assert.ok(html.indexOf("<h1>") < html.indexOf("Share this view"), "the title leads and the primary action ends the header row");
  assert.ok(html.indexOf("Share this view") < html.indexOf("Search this view"));
  assert.doesNotMatch(html, /autofocus/i);
  assert.ok(html.indexOf("current-answer") < html.indexOf("Session details"));
  assert.match(html, /<span class="sv-details-synopsis">2 messages<\/span>/, "session details summarize the capture at the end of the page");
  assert.doesNotMatch(html, /Explore trace and raw data|class="sv-facts"/, "details stay closed and nothing unfolds inline");
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

test("sharing: a rendered selection maps back to its source passage", async () => {
  const { sourceRange, partUnit } = await sessionModel();
  const source = "## Recommendation\n\nCompare the two onboarding paths.\n\n| Product | Setup |\n| --- | --- |\n| Atlas | **Self serve** |\n\n- Read the [captured documentation](https://example.test/docs).\n\nRepeat me. Then repeat me.";
  const passage = (selected, hint) => { const range = sourceRange(source, selected, hint); return range && source.slice(range.start, range.end); };
  assert.equal(passage("Compare the two  onboarding\npaths."), "Compare the two onboarding paths.");
  assert.equal(passage("Self serve"), "**Self serve**");
  assert.equal(passage("Atlas\tSelf serve"), "Atlas | **Self serve**");
  assert.equal(passage("Read the captured documentation"), "Read the [captured documentation](https://example.test/docs)");
  assert.equal(passage("captured documentation"), "[captured documentation](https://example.test/docs)");
  assert.equal(sourceRange(source, "me.", 0.99).start, source.lastIndexOf("me."));
  assert.equal(sourceRange(source, "me.", 0).start, source.indexOf("me."));
  assert.equal(sourceRange(source, "not in this text"), null);
  assert.equal(partUnit({ unitPrefix: "u2-out-0", partIndices: [0, 2] }, 1), "u2-out-0-2");
  assert.equal(partUnit({ unitPrefix: "u2-out-0", unitPrefixes: ["u2-out-0-5"] }, 0), "u2-out-0-5");
  const html = render(run([call("s1", [message("user", "Check it")], [message("assistant", "Checked")])]), "exchange", { source: "fixture" });
  assert.match(html, /data-unit="u1-in-0-0"/); assert.match(html, /data-unit="u1-out-0-0"/);
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

test("arrival: a Claude Code compaction summary and task notification read as context", async () => {
  const { exchangesFor, messageText, readingRole } = await sessionModel();
  const doc = JSON.parse(await readFile("testdata/import/claude-code/compaction/golden.run.json", "utf8"));
  const exchanges = exchangesFor(doc);
  assert.deepEqual(exchanges.map(e => e.prompts.map(p => messageText(p.msg))), [["Port the settings page to the new form components"], ["Continue with the notifications section"]]);
  const context = exchanges[1].blocks.filter(b => readingRole(b.msg) === "context").map(b => messageText(b.msg));
  assert.equal(context.length, 2);
  assert.match(context[0], /continued from a previous conversation/);
  assert.match(context[1], /task-notification/);
});

test("arrival: an imported pi session reads as the branch that was kept", async () => {
  const { exchangesFor, messageText } = await sessionModel();
  const doc = JSON.parse(await readFile("testdata/import/pi/branches/golden.run.json", "utf8"));
  assert.deepEqual(exchangesFor(doc).map(e => e.prompts.map(p => messageText(p.msg))), [["The upload test fails one run in five. Find out why."], ["Fix the race instead"], ["retry"]]);
  const html = render(doc, "conversation", { source: "fixture" });
  assert.match(html, /overloaded_error/);
  assert.doesNotMatch(html, /Deleted upload\.test\.ts/);
});

for (const [fixture, images] of [
  ["claude-code/images", 2], ["codex/images", 3], ["pi/images", 2], ["omp/v18.1-images", 2], ["opencode/v1.18-images", 2], ["hermes/stored-content", 1],
  ["dsh/v4-images", 1],
]) {
  test(`viewer: images imported from ${fixture} are shown`, async () => {
    const doc = JSON.parse(await readFile(`testdata/import/${fixture}/golden.run.json`, "utf8"));
    const html = render(doc);
    const shown = new Set(html.match(/<img[^>]+src="data:image\/(?:png|jpeg|webp);base64,[^"]+"/g) ?? []);
    assert.equal(shown.size, images, `distinct images rendered from ${fixture}`);
    assert.doesNotMatch(html, /no source|inline bytes omitted/);
  });
}

for (const [fixture, task] of [
  ["claude-code/sub-agent", /SUBTASK: print one line/], ["codex/sub-agent", /NEW_TASK/], ["omp/v18.1-sub-agent", /SUBTASK: print one line/],
  ["opencode/v1.18-sub-agent", /Reply with the single word done/], ["hermes/delegate", /List certificates in eu-west/],
  ["dsh/v4-sub-agents", /SUBTASK: print one line/],
]) {
  test(`arrival: a sub-agent imported from ${fixture} reads as a delegated task`, async () => {
    const { exchangesFor, messageText } = await sessionModel();
    const doc = JSON.parse(await readFile(`testdata/import/${fixture}/golden.run.json`, "utf8"));
    const exchanges = exchangesFor(doc);
    const delegated = exchanges.filter(e => e.child);
    assert.ok(delegated.length >= 1, "the sub-agent's work is an exchange of its own");
    assert.match(messageText(delegated[0].prompts[0].msg), task);
    assert.ok(exchanges.some(e => !e.child && e.prompts.length), "the session's own exchanges remain");
    // The sub-agent's work follows the exchange that delegated it.
    assert.ok(exchanges.indexOf(delegated[0]) > exchanges.findIndex(e => !e.child));
    // The transcript marks where the sub-agent's work begins, by its name.
    const agent = doc.spans.find(s => s.type === "agent" && s.parent_id);
    assert.ok(render(doc).includes(`aria-label="Inspect ${agent.name} in the tree"`));
  });
}

test("arrival: a dsh session reads as the person's turns, with what dsh supplied as context", async () => {
  const { exchangesFor, messageText } = await sessionModel();
  const doc = JSON.parse(await readFile("testdata/import/dsh/v4-run/golden.run.json", "utf8"));
  const exchanges = exchangesFor(doc);
  // dsh puts its runtime context in the person's turn; it is not a prompt of theirs.
  assert.deepEqual(exchanges.map(e => e.prompts.map(p => messageText(p.msg))),
    [["RUN_TOOL BAD_ARGS: print a greeting with the shell and report what it printed"], ["FOLLOW_UP: thanks, that is all"]]);
  const html = render(doc);
  assert.match(html, /hello from the tool/);
  assert.match(html, /missing required property/);
  // A request the provider failed is shown where it happened, before the retry that answered.
  const retried = JSON.parse(await readFile("testdata/import/dsh/v4-retry/golden.run.json", "utf8"));
  assert.match(render(retried), /mock overload \(SERVER\)[\s\S]*Hello\. Nothing to run here\./);
});

test("arrival: an aider run reads as its prompts and replies, with aider's own notices as evidence", async () => {
  const { exchangesFor, messageText, defaultExchange } = await sessionModel();
  const doc = JSON.parse(await readFile("testdata/import/aider/v0.86-edit/golden.run.json", "utf8"));
  const exchanges = exchangesFor(doc);
  // A command to aider is something the person typed, though no model answered it.
  assert.deepEqual(exchanges.map(e => e.prompts.map(p => messageText(p.msg))),
    [["BREAK_IT: make hello.py greet the world"], ["/run python hello.py"], ["FOLLOW_UP: thanks, that is all"], ["/exit"]]);
  // Reading opens on the last exchange that was answered, not on the closing /exit.
  assert.equal(defaultExchange(exchanges), exchanges[2]);
  const edit = exchanges[0].blocks.map(b => b.msg.role);
  // What aider said at startup is the setup the first prompt reads with. Then
  // the reply, what aider did with it, the fix the lint failure led to, and
  // what aider did with that.
  assert.deepEqual(edit, ["system", "assistant", "tool", "assistant", "tool"]);
  assert.match(messageText(exchanges[0].blocks[0].msg), /Aider v0\.86\.2[\s\S]*Added hello\.py to the chat\./);
  assert.match(exchanges[0].blocks[2].msg.content[0].content[0].text, /Applied edit to hello\.py[\s\S]*SyntaxError: '\(' was never closed/);
  assert.match(messageText(exchanges[0].blocks[3].msg), /The closing bracket was missing/);
  assert.match(render(doc), /Commit 06b6ca0 feat: greet the world/);
  // Aider records one time, the start of the run. Nothing else is given a time.
  assert.equal(doc.spans.filter(s => s.started_at).length, 1);
  assert.equal(doc.spans.filter(s => s.ended_at).length, 0);
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

test("reading: a tool span's own copy of a result defers to the result the model received", async () => {
  const { buildFlow } = await sessionModel();
  const toolCall = { role: "assistant", content: [{ type: "tool_call", id: "t1", name: "Bash", arguments: { command: "git status -sb" } }] };
  const received = { role: "tool", content: [{ type: "tool_result", tool_call_id: "t1", content: [{ type: "text", text: "model-saw-this" }] }] };
  const doc = run([
    call("s1", [message("user", "check git")], [toolCall]),
    { id: "t1", type: "tool_call", parent_id: "root", name: "Bash", input: { tool_call_id: "t1", arguments: { command: "git status -sb" } }, output: { result: { stdout: "model-saw-this", interrupted: false } } },
    call("s2", [received], [message("assistant", "clean tree")]),
  ], { kind: "import", harness: "claude-code", fidelity: "reconstructed" });
  const results = buildFlow(doc).filter(block => block.msg?.content.some(part => part.type === "tool_result"));
  assert.equal(results.length, 1, "one result per call");
  assert.equal(results[0].msg.content[0].content[0].text, "model-saw-this");
  assert.doesNotMatch(render(doc, "transcript"), /interrupted/);
});

test("reading: a failed tool's result stands in for its bare error status", async () => {
  const { buildFlow } = await sessionModel();
  const failing = (extra = {}) => ({ id: "t1", type: "tool_call", parent_id: "root", name: "Bash", status: "error", input: { tool_call_id: "t1", arguments: { command: "false" } }, ...extra });
  const errors = doc => buildFlow(doc).filter(block => block.err).map(block => block.err);
  const withResult = run([call("s1", [message("user", "fail")], []), failing({ output: { result: "Error: Exit code 1" } })]);
  assert.deepEqual(errors(withResult), [], "the failed result already says so");
  const withoutResult = run([call("s1", [message("user", "fail")], []), failing()]);
  assert.deepEqual(errors(withoutResult), ["Recorded error"], "a failure with no result stays visible");
  const withMessage = run([call("s1", [message("user", "fail")], []), failing({ output: { result: "x" }, error: { message: "timed out" } })]);
  assert.deepEqual(errors(withMessage), ["timed out"]);
});

test("reading: recorded tool input and output are verbatim, never Markdown", () => {
  const output = "## main...origin/main [ahead 2]\n  * indented, not a list\n<b>raw</b>";
  const doc = run([
    call("s1", [message("user", "push")], [
      { role: "assistant", content: [{ type: "tool_call", id: "t1", name: "Bash", arguments: { command: "git status -sb\ngit push", description: "Push main", timeout: 900000 } }] },
      { role: "tool", content: [{ type: "tool_result", tool_call_id: "t1", content: [{ type: "text", text: output }] }] },
    ]),
  ]);
  const html = render(doc, "transcript");
  assert.match(html, /<pre class="rv-tool-text">## main\.\.\.origin\/main \[ahead 2\]\n  \* indented, not a list\n&lt;b&gt;raw&lt;\/b&gt;<\/pre>/);
  assert.doesNotMatch(html, /<h2[^>]*>main/);
  assert.match(html, /<pre class="rv-tool-text rv-tool-command">git status -sb\ngit push<\/pre>/, "a shell command reads as the command it ran");
  assert.match(html, /→ Bash<\/span><span class="rv-tool-note">Push main<\/span>/);
  assert.match(html, /<dt>timeout<\/dt><dd>900000<\/dd>/);
  assert.match(html, /← Bash result/, "a result names its tool");
  assert.match(html, /title="Tool call t1"/); assert.doesNotMatch(html, />t1</, "call IDs stay out of the visible text");
});

test("arrival: a closing exit with no answer is not where reading starts", () => {
  const doc = run([
    call("s1", [message("user", "Compare the onboarding flows")], [message("assistant", "the-real-answer")]),
    { id: "tail", parent_id: "root", type: "custom", input: { messages: [message("user", "exit")] } },
  ], { kind: "import" });
  const html = render(doc, "exchange");
  assert.match(html, /the-real-answer/);
  assert.match(html, /<strong>1<\/strong> <span class="sv-of">of (?:<!-- -->)?2<\/span>/, "the exit stays in the conversation");
  doc.spans[2].input.messages = [message("user", "and the pricing?")];
  assert.match(render(doc, "exchange"), /No response captured/, "a real trailing question still leads");
});

test("navigation: reading continues at the end of an exchange", () => {
  const doc = run([
    call("s1", [message("user", "first-question")], [message("assistant", "first-answer")]),
    call("s2", [message("user", "second-question")], [message("assistant", "second-answer")]),
  ]);
  const local = render(doc, "exchange", { source: "fixture" });
  const nav = local.slice(local.indexOf('aria-label="Continue reading"'));
  assert.match(nav, /Previous<\/span><span class="sv-endnav-text">first-question</);
  assert.doesNotMatch(local, /first-answer/);
  assert.doesNotMatch(nav, /Copy link/, "local addresses are not for sharing");
  assert.match(render(doc, "exchange"), /aria-label="Continue reading"[\s\S]*Copy link/, "hosted readers can link to what they read");
});

test("navigation: an attached image's file tag is not the prompt's text", async () => {
  const { exchangesFor, promptLabel } = await sessionModel();
  const tag = { type: "text", text: '<image name=[Image #1] path="/tmp/Screenshot.png">' };
  const doc = run([
    call("s1", [{ role: "user", content: [tag, { type: "text", text: "Why is this layout broken?" }] }], [message("assistant", "a")]),
    call("s2", [{ role: "user", content: [tag] }], [message("assistant", "b")]),
  ], { kind: "import" });
  assert.deepEqual(exchangesFor(doc).map(promptLabel), ["Why is this layout broken?", "Image"]);
});

test("search: messages read as the page shows them, not as stored JSON", async () => {
  const { readableText } = await sessionModel();
  const prose = readableText({ role: "assistant", content: [{ type: "text", text: "## Plan\n**Try** the [docs](https://example.test/docs).\n\n| Path | Setup |\n| --- | --- |\n| Atlas | Self serve |" }] });
  assert.equal(prose.replace(/\s+/g, " ").trim(), "Plan Try the docs https://example.test/docs. Path Setup Atlas Self serve");
  const call = readableText({ role: "assistant", content: [{ type: "tool_call", id: "t1", name: "Bash", arguments: { command: "git status -sb", description: "Check the tree" } }] });
  assert.equal(call, "Bash git status -sb\nCheck the tree");
  const result = readableText({ role: "tool", content: [{ type: "tool_result", tool_call_id: "t1", content: [{ type: "text", text: "## main...origin/main\n | kept | verbatim |" }] }] });
  assert.equal(result, "## main...origin/main\n | kept | verbatim |", "recorded output is not rewritten");
  assert.doesNotMatch(prose + call + result, /"type"|\\n/);
});

test("navigation: previews lead with the prose before a table", async () => {
  const { previewText } = await sessionModel();
  assert.equal(previewText("## Recommendation\n\nCompare both paths.\n\n| Product | Setup |\n| --- | --- |\n| Atlas | Self serve |", 200), "Recommendation Compare both paths.");
});

test("reading: a subagent's input is a delegated task, not human input", () => {
  const doc = run([
    { id: "child", parent_id: "root", type: "agent", name: "Researcher" },
    call("c1", [message("user", "child-task")], [message("assistant", "child-result")], { parent_id: "child" }),
  ]);
  const html = render(doc, "exchange");
  assert.match(html, /child-task/);
  assert.match(html, />Delegated task</);
  assert.doesNotMatch(html, />Human input</);
});

test("navigation: each subagent's exchanges count among themselves", () => {
  const doc = run([
    { id: "a", parent_id: "root", type: "agent", name: "Planner" },
    call("a1", [message("user", "plan")], [message("assistant", "planned")], { parent_id: "a" }),
    { id: "b", parent_id: "root", type: "agent", name: "Reviewer" },
    call("b1", [message("user", "review one")], [message("assistant", "one")], { parent_id: "b" }),
    call("b2", [message("user", "review two")], [message("assistant", "two")], { parent_id: "b" }),
  ]);
  assert.match(render(doc, "exchange"), /<span class="sv-agent">Reviewer<\/span> <strong>2<\/strong> <span class="sv-of">of (?:<!-- -->)?2<\/span>/);
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

test("reading: human input, responses and activity show their recorded times", async () => {
  const { elapsed } = await sessionModel();
  const doc = run([
    call("s1", [message("user", "Check the build")], [{ role: "assistant", content: [{ type: "tool_call", id: "t1", name: "shell", arguments: { command: "make" } }] }], { started_at: "2026-09-11T13:04:00.000Z", ended_at: "2026-09-11T13:04:05.000Z" }),
    call("s2", [{ role: "tool", content: [{ type: "tool_result", tool_call_id: "t1", content: [{ type: "text", text: "ok" }] }] }], [message("assistant", "The build passes.")], { started_at: "2026-09-11T13:05:30.000Z", ended_at: "2026-09-11T13:06:08.000Z" }),
  ]);
  const html = render(doc, "exchange");
  assert.match(html, /Human input<time class="sv-time" dateTime="2026-09-11T13:04:00\.000Z" title="[^"]+">[^<]+<\/time>/);
  assert.match(html, /Agent response<time class="sv-time" dateTime="2026-09-11T13:06:08\.000Z" title="[^"]+">[^<]+<\/time>/);
  assert.match(html, /<summary>Agent activity(?:<!-- -->)? · 2 steps<span class="sv-duration" title="[^"]+"> · (?:<!-- -->)?2m 8s<\/span><\/summary>/);
  assert.deepEqual([["13:04:00", "13:04:45"], ["13:04:00", "13:16:30"], ["13:04:00", "14:09:10"], ["13:04:00", "13:04:00"]].map(([a, b]) => elapsed(`2026-09-11T${a}Z`, `2026-09-11T${b}Z`)), ["45s", "12m", "1h 5m", ""]);
  assert.equal(elapsed(undefined, "2026-09-11T13:04:00Z"), "");
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
  "testdata/import/codex/tool-kinds/golden.run.json",
  "testdata/import/claude-code/basic/golden.run.json",
  "testdata/import/claude-code/split-response/golden.run.json",
  "testdata/import/claude-code/compaction/golden.run.json",
  "testdata/import/hermes/stored-content/golden.run.json",
  "testdata/import/opencode/v1.18-run/golden.run.json",
  "testdata/import/omp/v18.1-run/golden.run.json",
  "testdata/import/opencode/v1.18-images/golden.run.json",
  "testdata/import/claude-code/sub-agent/golden.run.json",
  "testdata/import/codex/sub-agent/golden.run.json",
  "testdata/import/pi/error-and-trailing/golden.run.json",
  "testdata/import/dsh/v4-run/golden.run.json",
  "testdata/import/dsh/v4-retry/golden.run.json",
  "testdata/import/dsh/v4-sub-agents/golden.run.json",
  "testdata/import/aider/v0.86-edit/golden.run.json",
  "testdata/import/aider/v0.86-two-runs/golden.run.json",
]) {
  test(`viewer: renders ${fixture}`, async () => {
    const doc = JSON.parse(await readFile(fixture, "utf8"));
    assert.match(render(doc), /aria-label="Viewer mode"/);
  });
}
