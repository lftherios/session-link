#!/usr/bin/env node
// Local integration check. Requires a built slink and a Chromium-family browser.
// Uses only fictional fixtures, a temporary SLINK_HOME and an isolated profile.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { mkdtemp, writeFile, readFile } from "node:fs/promises";
import { fileURLToPath } from "node:url";
import path from "node:path";
import os from "node:os";

const root = fileURLToPath(new URL("../", import.meta.url));
const home = await mkdtemp(path.join(os.tmpdir(), "slink-arrival-"));
console.log(`Browser artifacts: ${home}`);
const cli = spawn(process.env.SLINK_BINARY ?? "/tmp/session-link-slink", ["view", "--session", path.join(root, "testdata/viewer/arrival/session.json"), "--no-browser"], {
  cwd: root, env: { ...process.env, SLINK_HOME: home, SLINK_SERVER: "http://127.0.0.1:1" }, stdio: ["ignore", "pipe", "pipe"],
});
let browser, socket;
const waitOutput = (child, stream, pattern) => new Promise((resolve, reject) => {
  let output = ""; const timer = setTimeout(() => reject(new Error(`Startup timed out: ${output.slice(-500)}`)), 15000);
  child[stream].on("data", chunk => { output += chunk; const match = output.match(pattern); if (match) { clearTimeout(timer); resolve(match[0]); } });
  child.once("error", error => { clearTimeout(timer); reject(error); });
  child.once("exit", code => { clearTimeout(timer); reject(new Error(`Process exited ${code}: ${output.slice(-500)}`)); });
});
try {
  const url = await waitOutput(cli, "stdout", /http:\/\/127\.0\.0\.1:\d+\/p\/[\w.-]+/);
  const base = new URL(url).origin, source = new URL(url).pathname.split("/").pop();
  browser = spawn(process.env.BROWSER_BINARY ?? "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser", ["--headless", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--disable-background-networking", "--disable-component-update", "--disable-sync", "--remote-debugging-port=0", `--user-data-dir=${path.join(home, "browser")}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
  const endpoint = await waitOutput(browser, "stderr", /ws:\/\/[^\s]+/);
  const targets = await (await fetch(`http://127.0.0.1:${new URL(endpoint).port}/json/list`)).json();
  socket = new WebSocket(targets.find(target => target.type === "page").webSocketDebuggerUrl);
  await new Promise((resolve, reject) => { socket.onopen = resolve; socket.onerror = reject; });
  let seq = 0; const pending = new Map(), errors = [];
  socket.onmessage = ({ data }) => {
    const message = JSON.parse(data);
    if (message.id) { const call = pending.get(message.id); pending.delete(message.id); message.error ? call.reject(new Error(JSON.stringify(message.error))) : call.resolve(message.result); }
    if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails.text);
  };
  const send = (method, params = {}) => new Promise((resolve, reject) => { const id = ++seq; pending.set(id, { resolve, reject }); socket.send(JSON.stringify({ id, method, params })); });
  const evaluate = async expression => { const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true }); if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails)); return result.result.value; };
  const waitFor = async expression => { for (let i = 0; i < 160; i++) { if (await evaluate(`!window.__RELOADING__ && (${expression})`)) return; await new Promise(resolve => setTimeout(resolve, 50)); } throw new Error(`Timed out: ${expression}`); };
  const clickText = text => evaluate(`Array.from(document.querySelectorAll('button,a,summary')).find(el=>el.textContent===${JSON.stringify(text)}).click()`);
  const setField = (selector, value) => evaluate(`(()=>{const el=document.querySelector(${JSON.stringify(selector)});const proto=el instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype;Object.getOwnPropertyDescriptor(proto,'value').set.call(el,${JSON.stringify(value)});el.dispatchEvent(new Event('input',{bubbles:true}));})()`);
  const reload = async () => { await evaluate("window.__RELOADING__=true"); await send("Page.reload"); };
  const navigate = async target => { await evaluate("window.__RELOADING__=true"); const result = await send("Page.navigate", { url: target }); if (!result.loaderId) await evaluate("window.__RELOADING__=false"); };
  const screenshot = async name => { const { data } = await send("Page.captureScreenshot", { format: "png" }); await writeFile(path.join(home, `${name}.png`), Buffer.from(data, "base64")); };
  await send("Runtime.enable"); await send("Page.enable");
  await send("Emulation.setDeviceMetricsOverride", { width: 1280, height: 950, deviceScaleFactor: 1, mobile: false });
  await navigate(url); await waitFor("!!document.querySelector('.sv-response-body table')");
  assert.match(await evaluate("document.querySelector('.sv-position').textContent"), /^8 of 8$/);
  assert.match(await evaluate("document.querySelector('.sv-prompt').textContent"), /Write the recommendation/);
  assert.equal(await evaluate("document.activeElement.tagName"), "BODY");
  assert.equal(await evaluate("document.querySelector('.sv-details').open"), false);
  assert.equal(await evaluate("!!document.querySelector('.session-actions')"), false);
  assert.equal(await evaluate("/exchange/i.test(document.querySelector('.sv').innerText)"), false);
  assert.equal(await evaluate("document.querySelector('.sv-reading').textContent.includes('Child result')"), false);
  assert.equal(await evaluate("document.querySelectorAll('.sv-primary').length"), 1);
  assert.equal(await evaluate("!!document.querySelector('.sv-block-tools, .sv-block-menu, .sv-select, .sv-selection-bar')"), false, "messages carry no per-message controls");
  assert.ok(await evaluate("document.querySelector('.sv-toolbar').getBoundingClientRect().height<50"));
  await evaluate("document.querySelector('.sv-position').click()"); await waitFor("!!document.querySelector('#sv-outline [aria-current=true]')");
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
  await waitFor("!document.querySelector('#sv-outline') && document.activeElement.classList.contains('sv-position')");
  await evaluate("document.activeElement.blur()");
  assert.ok(await evaluate("document.querySelector('.sv-search').getBoundingClientRect().bottom<innerHeight/2"));
  assert.equal(await evaluate("!!document.querySelector('.sv-selection-bar')"), false);
  assert.equal(await evaluate("document.querySelectorAll('.sv-support').length"), 1);
  assert.equal(await evaluate("document.querySelector('.sv-support').open"), false);
  assert.equal(await evaluate("document.querySelectorAll('.sv-thinking').length"), 0);
  assert.equal(await evaluate("document.querySelector('.sv-response-body').textContent.includes('thinking-evidence')"), false);
  await screenshot("arrival-desktop");
  console.log("PASS: latest input and answer, prominent search, compact navigation, one share CTA, no per-message controls");

  await evaluate("document.querySelector('.sv-support>summary').click()");
  await waitFor("document.querySelectorAll('.sv-support .sv-thinking').length===5");
  assert.ok(await evaluate("Array.from(document.querySelectorAll('.sv-support .sv-thinking')).every(el=>el.getClientRects().length>0)"));
  assert.equal(await evaluate("Array.from(document.querySelectorAll('.sv-support summary')).some(el=>el.textContent.trim()==='thinking')"), false);
  assert.equal(await evaluate("document.querySelectorAll('.sv-reasoning-unavailable').length"), 1);
  assert.match(await evaluate("document.querySelector('.sv-reasoning-unavailable').textContent"), /encrypted reasoning without a readable summary/);
  assert.equal(await evaluate("document.querySelector('.sv-support').textContent.includes('[reasoning]')"), false);
  assert.ok(await evaluate("Array.from(document.querySelectorAll('.sv-step .sv-block')).every(el=>el.textContent.trim().length>0)"));
  await evaluate("document.querySelector('.sv-support').scrollIntoView({block:'start'})");
  await screenshot("activity-expanded");
  await evaluate("document.querySelector('.sv-support>summary').click()");
  await waitFor("document.querySelectorAll('.sv-thinking').length===0");
  await navigate(url + "#message=u8-out-0");
  await waitFor("!!document.querySelector('.sv-reasoning-unavailable.sv-linked')");
  assert.equal(await evaluate("document.querySelectorAll('.sv-step .sv-thinking').length"), 5);
  assert.equal(await evaluate("document.querySelector('.sv-support').textContent.includes('[reasoning]')"), false);
  await navigate(url + "#span=s8"); await waitFor("!document.querySelector('.sv-support').open");
  await evaluate("window.scrollTo(0,0)");
  await setField('[aria-label="Search session content"]', "thinking-evidence-5");
  await waitFor("document.querySelectorAll('#sv-search-results .sv-outline-list>button').length===1");
  await evaluate("document.querySelector('#sv-search-results .sv-outline-list>button').click()");
  await waitFor("document.querySelector('.sv-support').open");
  assert.match(await evaluate("document.querySelector('.sv-linked').textContent"), /thinking-evidence-5/);
  assert.equal(await evaluate("document.querySelector('.sv-response-body').textContent.includes('thinking-evidence')"), false);
  await navigate(url + "#span=s8"); await waitFor("!!document.querySelector('.sv-response-body table') && !document.querySelector('.sv-support').open");
  console.log("PASS: one activity section shows readable reasoning and a single missing-text note; legacy placeholders stay out of the reader and links still resolve");

  await setField('[aria-label="Search session content"]', "Finding 7");
  await waitFor("document.querySelector('#sv-search-results [role=status]').textContent.includes('0 matches')");
  await clickText("Whole session");
  await waitFor("document.querySelectorAll('#sv-search-results .sv-outline-list>button').length===1");
  await evaluate("document.querySelector('#sv-search-results .sv-outline-list>button').click()");
  await waitFor("document.querySelector('.sv-response-body').textContent.includes('Finding 7')");
  await reload(); await waitFor("!!document.querySelector('.sv-response-body')");
  assert.match(await evaluate("document.querySelector('.sv-response-body').textContent"), /Finding 7/);
  await navigate(url + "#span=s8"); await waitFor("!!document.querySelector('.sv-response-body table')");
  await setField('[aria-label="Search session content"]', "source-check-42");
  await waitFor("document.querySelectorAll('#sv-search-results .sv-outline-list>button').length===1");
  await evaluate("document.querySelector('#sv-search-results .sv-outline-list>button').click()");
  await waitFor("document.querySelector('.sv-support').open");
  assert.match(await evaluate("document.querySelector('.sv-linked').textContent"), /source-check-42/);
  console.log("PASS: view search stays scoped; session search reaches earlier answers; view search reveals collapsed activity");

  await clickText("Full session"); await waitFor("!!document.querySelector('.sv-conversation-card')");
  assert.ok(await evaluate("document.querySelectorAll('.sv-conversation-card').length>=8 && ![...document.querySelectorAll('button')].some(b=>/^Show (earlier|later)/.test(b.textContent))"), "the full session renders every exchange");
  const shown = await evaluate("parseInt(document.querySelector('.sv-position strong').textContent)");
  const step = shown > 1 ? "Previous" : "Next", target = shown > 1 ? shown - 1 : shown + 1;
  await evaluate(`document.querySelector('[aria-label="${step} in conversation"]').click()`);
  await waitFor(`document.querySelector('.sv-position strong').textContent==='${target}' && !!document.querySelector('.sv-conversation-card') && document.querySelector('[aria-label="Reading layout"] [aria-pressed="true"]').textContent==='Full session'`);
  assert.equal(await evaluate("[...document.querySelectorAll('.sv-conversation-card button')].some(b=>b.textContent.trim()==='Focus') || !!document.querySelector('.sv-conversation-card > .sv-label button')"), false, "cards carry no Focus buttons");
  await clickText("Raw data"); await waitFor("!!document.querySelector('.sv-raw-dialog[open]')");
  assert.equal(await evaluate("JSON.parse(document.querySelector('.sv-raw').textContent).spans.length===window.__RUN__.spans.length"), true);
  await send("Input.dispatchKeyEvent", { type: "keyDown", key: "Escape", code: "Escape", windowsVirtualKeyCode: 27 });
  await waitFor("!document.querySelector('.sv-raw-dialog') && document.activeElement.classList.contains('sv-raw-button')");
  await clickText("Focused");
  await waitFor("!document.querySelector('.sv-conversation-card')");
  await navigate(url + "#span=s8"); await waitFor("!!document.querySelector('.sv-response-body table')");
  await evaluate("history.replaceState(null,'',location.pathname);window.scrollTo(0,450)");
  const y = await evaluate("scrollY");
  // A link to a span with no conversation opens the inspector over the page.
  await evaluate("location.hash='span=child'");
  await waitFor("!!document.querySelector('.sv-dialog[open]')");
  await clickText("Raw data"); await clickText("Close inspection");
  assert.equal(await evaluate("scrollY"), y);
  await evaluate("history.replaceState(null,'',location.pathname)");
  await reload(); await waitFor("!!document.querySelector('.sv-response-body table')");
  await waitFor(`Math.abs(scrollY-${y})<3`);
  console.log("PASS: conversation navigation, explicit links, detail return and reading position survive");

  await evaluate("window.scrollTo(0,0)");
  await clickText("Share this view"); await waitFor("document.querySelectorAll('.sv-included-item').length===2");
  assert.equal(await evaluate("document.querySelector('.sv-author-fields').open"), false);
  assert.equal(await evaluate("document.querySelectorAll('.sv-included-item')[0].textContent.includes('Human input')"), true);
  assert.equal(await evaluate("document.querySelector('.sv-included-item').textContent.includes('opens first')"), true);
  await screenshot("share-view");
  await evaluate("window.__realFetch=window.fetch;window.fetch=(...args)=>String(args[0]).startsWith('/api/export/')?Promise.resolve(new Response(JSON.stringify({error:{message:'Temporary save failure'}}),{status:503,headers:{'content-type':'application/json'}})):window.__realFetch(...args)");
  await clickText("Save locally"); await waitFor("document.querySelector('[role=alert]')?.textContent.includes('Temporary save failure')");
  assert.equal(await evaluate("document.querySelectorAll('.sv-included-item').length"), 2);
  await evaluate("window.fetch=window.__realFetch");
  await clickText("Save locally"); await waitFor("document.querySelector('.sv-saved-notice')?.textContent.includes('View saved locally')");
  const savedURL = await evaluate("document.querySelector('.sv-saved-notice a').href");
  const savedID = new URL(savedURL).pathname.split('/').pop();
  const saved = JSON.parse(await readFile(path.join(home, 'previews', savedID + '.json'), 'utf8'));
  assert.equal(saved.spans.length, 3);
  assert.equal(saved.spans[1].input.messages[0].role, 'user');
  assert.equal(saved.extensions['session_link.share.v1'].primary_id, saved.spans[1].id);
  assert.equal(JSON.stringify(saved).includes('source-check-42'), false);
  assert.equal(JSON.stringify(saved).includes('Finding 7'), false);
  assert.equal(JSON.stringify(saved).includes('thinking-evidence'), false);
  await evaluate("document.querySelector('[aria-label=\"Close share panel\"]').click()");
  await reload(); await waitFor("!!document.querySelector('.sv-share')");
  await clickText("Share this view"); await waitFor("!!document.querySelector('.sv-saved-views a')");
  assert.equal(await evaluate("document.querySelector('.sv-saved-views a').href"), savedURL);
  await evaluate("document.querySelector('[aria-label=\"Close share panel\"]').click()");
  console.log("PASS: share panel saves exactly the human input and answer locally; saved views remain discoverable after reload");

  // Title and comment persist while the reader stays open.
  await evaluate("window.scrollTo(0,0)");
  await clickText("Share this view"); await waitFor("document.querySelectorAll('.sv-included-item').length===2");
  assert.equal(await evaluate("document.querySelector('.sv-included').textContent.includes('thinking-evidence')"), false);
  await evaluate("document.querySelector('.sv-author-fields').open=true");
  await setField('[aria-label="Your comment"]', "Please review the recommendation and the captured evidence.");
  await setField('[aria-label="View title"]', "Review the onboarding comparison");
  await evaluate("document.querySelector('[aria-label=\"Close share panel\"]').click()");
  await clickText("Share this view"); await waitFor("!!document.querySelector('[aria-label=\"Your comment\"]')");
  assert.equal(await evaluate("document.querySelector('[aria-label=\"Your comment\"]').value"), "Please review the recommendation and the captured evidence.");
  await clickText("Preview view"); await waitFor("!!document.querySelector('.sh-primary') && !!document.querySelector('.sh-card table')");
  assert.match(await evaluate("document.querySelector('.sh-primary').textContent"), /Human input/);
  assert.equal(await evaluate("window.__RUN__.spans.length"), 4);
  assert.equal(await evaluate("JSON.stringify(window.__RUN__).includes('Earlier recovered failure')"), false);
  console.log("PASS: the comment persists while the reader stays open; preview contains only chosen material");

  await navigate(url + "#span=s8"); await waitFor("!!document.querySelector('.sv-response-body table')");
  await clickText("Share this view"); await waitFor("document.querySelectorAll('.sv-included-item').length===2");
  // Keep only the agent response, narrowed to an exact passage.
  await evaluate("(()=>{const input=[...document.querySelectorAll('.sv-included-item')].find(el=>el.querySelector('summary').textContent.includes('Human input'));[...input.querySelectorAll('button')].find(b=>b.textContent==='Remove').click();})()");
  await waitFor("document.querySelectorAll('.sv-included-item').length===1");
  await evaluate("document.querySelector('.sv-included-item').open=true");
  await clickText("Select passage"); await waitFor("!!document.querySelector('.sv-passage')");
  const chosenPassage = await evaluate("(()=>{const el=document.querySelector('.sv-passage');const start=el.value.indexOf('Atlas');el.focus();el.setSelectionRange(start,start+5);el.dispatchEvent(new KeyboardEvent('keyup',{key:'Shift',bubbles:true}));return el.value.slice(start,start+5);})()");
  await waitFor("Array.from(document.querySelectorAll('button')).find(el=>el.textContent==='Use selected passage')?.disabled===false");
  await clickText("Use selected passage"); await waitFor("!document.querySelector('.sv-passage')");
  await clickText("Save locally");
  await waitFor("!!document.querySelector('.sv-saved-notice a')");
  const passageURL = await evaluate("document.querySelector('.sv-saved-notice a').href");
  const passageDoc = JSON.parse(await readFile(path.join(home, 'previews', new URL(passageURL).pathname.split('/').pop() + '.json'), 'utf8'));
  assert.equal(passageDoc.spans[1].input.messages[0].content[0].text, chosenPassage);
  assert.equal(passageDoc.extensions['session_link.share.v1'].items[0].passage, true);
  console.log("PASS: the share panel narrows to an exact passage and saved passages retain exact source characters");

  // Right-clicking selected text comments on and shares exactly that passage.
  await evaluate("document.querySelector('[aria-label=\"Close share panel\"]')?.click()"); await waitFor("!document.querySelector('.sv-share-dialog')");
  await navigate(url + "#span=s8"); await waitFor("!!document.querySelector('.sv-response-body table')");
  assert.equal(await evaluate("(()=>{getSelection().removeAllRanges();const el=document.querySelector('.sv-response-body p');const b=el.getBoundingClientRect();return el.dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true,clientX:b.left+4,clientY:b.top+4}));})()"), true, "without a selection the browser menu stays");
  assert.equal(await evaluate("!!document.querySelector('.sv-context-menu')"), false);
  const selected = await evaluate("(()=>{const li=[...document.querySelectorAll('.sv-response li')].find(el=>el.textContent.startsWith('Read the'));const walker=document.createTreeWalker(li,NodeFilter.SHOW_TEXT);const nodes=[];while(walker.nextNode())nodes.push(walker.currentNode);const first=nodes.find(n=>n.textContent.includes('Read the'));const link=li.querySelector('a').firstChild;const r=document.createRange();r.setStart(first,first.textContent.indexOf('Read the'));r.setEnd(link,link.textContent.length);const s=getSelection();s.removeAllRanges();s.addRange(r);const b=r.getClientRects()[0];const text=s.toString();li.dispatchEvent(new MouseEvent('contextmenu',{bubbles:true,cancelable:true,clientX:b.left+3,clientY:b.top+b.height/2}));return text;})()");
  assert.equal(selected, "Read the captured documentation");
  await waitFor("!!document.querySelector('.sv-context-menu')");
  assert.equal(await evaluate("getSelection().toString()"), "Read the captured documentation", "the selection stays highlighted while the menu is open");
  await clickText("Comment and share"); await waitFor("document.activeElement?.getAttribute('aria-label')==='Your comment'");
  assert.equal(await evaluate("document.querySelector('.sv-share-dialog h2').textContent"), "Comment and share");
  assert.equal(await evaluate("document.querySelectorAll('.sv-included-item').length"), 1);
  await setField('[aria-label="Your comment"]', "Is this the documentation we should trust?");
  await clickText("Save locally"); await waitFor("!!document.querySelector('.sv-saved-notice a')");
  const commentURL = await evaluate("document.querySelector('.sv-saved-notice a').href");
  const commentDoc = JSON.parse(await readFile(path.join(home, 'previews', new URL(commentURL).pathname.split('/').pop() + '.json'), 'utf8'));
  const commentItems = commentDoc.extensions['session_link.share.v1'].items;
  assert.equal(commentItems.length, 1); assert.equal(commentItems[0].passage, true);
  assert.ok(JSON.stringify(commentDoc).includes("Read the [captured documentation](https://example.test/docs)"));
  assert.ok(JSON.stringify(commentDoc).includes("Is this the documentation we should trust?"));
  console.log("PASS: right-clicking selected text comments on and shares exactly that passage");

  // Session details summarize the capture; raw data and the trace open on demand.
  await evaluate("document.querySelector('[aria-label=\"Close share panel\"]')?.click()"); await waitFor("!document.querySelector('.sv-share-dialog')");
  await evaluate("document.querySelector('.sv-details > summary').click()"); await waitFor("document.querySelectorAll('.sv-facts .sv-fact').length>=6");
  assert.equal(await evaluate("!!document.querySelector('.sv-details .rv-tree')"), false, "the trace explorer does not unfold inline");
  await clickText("Trace explorer"); await waitFor("!!document.querySelector('.sv-trace-dialog[open] .rv-tree')");
  await evaluate("document.querySelector('.sv-trace-dialog .sv-dialog-head button').click()");
  await waitFor("!document.querySelector('.sv-trace-dialog') && document.activeElement.textContent==='Trace explorer'");
  console.log("PASS: session details summarize the capture and open the trace explorer in a dialog");

  await navigate(url); await waitFor("!!document.querySelector('.sv h1')");
  await send("Emulation.setEmulatedMedia", { features: [{ name: "prefers-color-scheme", value: "dark" }, { name: "hover", value: "none" }] });
  await send("Emulation.setDeviceMetricsOverride", { width: 390, height: 844, deviceScaleFactor: 1, mobile: true });
  await send("Emulation.setTouchEmulationEnabled", { enabled: true });
  await evaluate("window.scrollTo(0,0)");
  assert.ok(await evaluate("document.documentElement.scrollWidth<=innerWidth"));
  await screenshot("arrival-mobile-dark");
  await clickText("Share this view"); await waitFor("!!document.querySelector('.sv-included-item')");
  assert.ok(await evaluate("document.querySelector('.sv-share-dialog').scrollWidth<=document.querySelector('.sv-share-dialog').clientWidth"));
  await screenshot("share-mobile-dark");
  assert.deepEqual(errors, []);
  console.log("PASS: mobile reading and sharing have no page overflow or browser exceptions");
  console.log(`Screenshots: ${home}`);
  const original = JSON.parse(await readFile(path.join(home, "previews", `${source}.json`), "utf8"));
  assert.equal(original.name, "Onboarding research");
  await fetch(base + "/api/stop", { method: "POST", headers: { "x-slink": "1" } });
} finally {
  socket?.close(); browser?.kill("SIGTERM"); cli.kill("SIGTERM");
}
