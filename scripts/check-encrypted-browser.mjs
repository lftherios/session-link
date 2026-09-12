#!/usr/bin/env node
// Invoked by the hosted service's smoke-encrypted.mjs against its fixture server.
import assert from "node:assert/strict";
import { existsSync } from "node:fs";
import { spawn } from "node:child_process";
import { mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";

const base = process.env.SLINK_BROWSER_BASE, link = process.env.SLINK_BROWSER_LINK;
if (!base || !link || !process.env.SLINK_BROWSER_MAIL) throw new Error("Run through smoke-encrypted.mjs with SLINK_BROWSER_SCRIPT set to this file");
const contentKey = new URLSearchParams(new URL(link).hash.slice(1)).get("key");
const dir = await mkdtemp(path.join(os.tmpdir(), "slink-encrypted-browser-"));
const candidates = [process.env.CHROME_PATH, path.join(os.homedir(), "Library/Caches/ms-playwright/chromium_headless_shell-1223/chrome-headless-shell-mac-arm64/chrome-headless-shell"), "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome", "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"];
const executable = candidates.find(p => p && existsSync(p));
if (!executable) throw new Error("Set CHROME_PATH to a Chromium-family browser executable");
const browser = spawn(executable, ["--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check", "--remote-debugging-port=0", `--user-data-dir=${dir}`, "about:blank"], { stdio:["ignore","pipe","pipe"] });
let socket;
try {
  const endpoint = await new Promise((resolve,reject) => {
    let text=""; const timeout=setTimeout(()=>reject(new Error("Browser startup timed out")),15_000);
    browser.stderr.on("data", chunk=>{ text+=chunk; const match=text.match(/DevTools listening on (ws:\/\/\S+)/); if(match){clearTimeout(timeout);resolve(match[1]);} });
    browser.on("error",reject);
  });
  const targets=await (await fetch(`http://127.0.0.1:${new URL(endpoint).port}/json/list`)).json();
  socket=new WebSocket(targets.find(t=>t.type==="page").webSocketDebuggerUrl);
  await new Promise((resolve,reject)=>{socket.onopen=resolve;socket.onerror=reject;});
  let seq=0; const pending=new Map(), errors=[], requests=[];
  socket.onmessage=({data})=>{
    const m=JSON.parse(data);
    if(m.id){const call=pending.get(m.id);pending.delete(m.id);m.error?call.reject(new Error(JSON.stringify(m.error))):call.resolve(m.result);}
    if(m.method==="Runtime.exceptionThrown")errors.push(m.params.exceptionDetails.text);
    if(m.method==="Network.requestWillBeSent")requests.push(m.params.request);
  };
  const send=(method,params={})=>new Promise((resolve,reject)=>{const id=++seq;pending.set(id,{resolve,reject});socket.send(JSON.stringify({id,method,params}));});
  const evaluate=async expression=>{const r=await send("Runtime.evaluate",{expression,awaitPromise:true,returnByValue:true});if(r.exceptionDetails)throw new Error(JSON.stringify(r.exceptionDetails));return r.result.value;};
  const waitFor=async expression=>{for(let i=0;i<160;i++){if(await evaluate(`!!document.body && (${expression})`))return;await new Promise(r=>setTimeout(r,50));}throw new Error(`Timed out: ${expression}`);};
  const navigate=async url=>{await send("Page.navigate",{url});};
  const screenshot=async name=>{const r=await send("Page.captureScreenshot",{format:"png"});await writeFile(path.join(dir,`${name}.png`),Buffer.from(r.data,"base64"));};
  await send("Runtime.enable");await send("Page.enable");await send("Network.enable");
  await send("Emulation.setDeviceMetricsOverride",{width:1280,height:950,deviceScaleFactor:1,mobile:false});
  await navigate(link);await waitFor("document.body.innerText.includes('PRIVATE_TITLE_SMOKE_SENTINEL')");
  assert.equal(await evaluate("document.body.innerText.includes('Encrypted share')"),true);
  await screenshot("encrypted-share");
  // Changing only the fragment must discard the old plaintext and retry.
  await evaluate(`location.hash='key='+${JSON.stringify("A".repeat(43))}`);
  await waitFor("document.body.innerText.includes('could not be decrypted')");
  assert.equal(await evaluate("document.body.innerText.includes('PRIVATE_TITLE_SMOKE_SENTINEL')"),false);
  await evaluate("location.hash=''");await waitFor("document.body.innerText.includes('missing its encryption key')");
  await screenshot("missing-key");
  await evaluate(`location.hash='key='+${JSON.stringify(contentKey)}+'&span=root'`);
  await waitFor("document.body.innerText.includes('PRIVATE_TITLE_SMOKE_SENTINEL')");
  assert.equal(await evaluate("new URLSearchParams(location.hash.slice(1)).get('key')"),contentKey);
  for(const req of requests){
    assert.equal(req.url.includes(contentKey),false,"key in HTTP URL");
    assert.equal(JSON.stringify(req.headers).includes(contentKey),false,"key in HTTP headers");
    assert.equal((req.postData??"").includes(contentKey),false,"key in HTTP body");
  }

  // Exercise the actual email UI with secure, HttpOnly cookies on localhost.
  await navigate(base+"/login");await waitFor("!!document.querySelector('input[name=email]')");
  await evaluate("document.querySelector('input[name=email]').value='browser@example.test';document.querySelector('form').requestSubmit()");
  await waitFor("!!document.querySelector('input[name=code]')");
  let email=await (await fetch(process.env.SLINK_BROWSER_MAIL)).json();
  const code=email.text.match(/code is (\d{8})/)[1];
  await evaluate(`document.querySelector('input[name=code]').value=${JSON.stringify(code)};document.querySelector('form').requestSubmit()`);
  await waitFor("location.pathname==='/account' && document.body.innerText.includes('browser@example.test')");
  await screenshot("email-account");
  await evaluate("document.querySelector('form[action=\"/api/auth/logout\"]').requestSubmit()");
  await waitFor("!!document.querySelector('input[name=email]')");
  await evaluate("document.querySelector('input[name=email]').value='browser@example.test';document.querySelector('form').requestSubmit()");
  await waitFor("!!document.querySelector('input[name=code]')");
  email=await (await fetch(process.env.SLINK_BROWSER_MAIL)).json();
  const magicLink=email.text.match(/http:\/\/[^\s]+#token=[\w-]+/)[0];
  await navigate(magicLink);await waitFor("!!document.querySelector('input[name=token]')");
  assert.match(await evaluate("location.pathname"),/^\/login\/email\//,"GET consumed magic link");
  await evaluate("document.querySelector('form').requestSubmit()");
  await waitFor("location.pathname==='/account' && document.body.innerText.includes('browser@example.test')");
  assert.deepEqual(errors,[]);
  console.log(`✓ Browser decryption, fragment-key changes, missing/wrong keys, email code and magic-link confirmation (${dir})`);
} finally { socket?.close();browser.kill("SIGTERM"); }
