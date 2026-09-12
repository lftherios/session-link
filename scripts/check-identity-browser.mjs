#!/usr/bin/env node
// Real first-share + recovery/device UI, using the smoke server's fake mailbox.
import "./check-encrypted-browser.mjs";
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, writeFile, readFile, readdir } from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
const root=fileURLToPath(new URL("../",import.meta.url)),base=process.env.SLINK_BROWSER_BASE;
const dir=await mkdtemp(path.join(os.tmpdir(),"slink-identity-browser-")),processes=[],sockets=[],errors=[],requests=[];
const waitOutput=(child,stream,pattern)=>new Promise((resolve,reject)=>{let text="";const timer=setTimeout(()=>reject(new Error(`Startup timed out: ${text.slice(-600)}`)),20000);child[stream].on("data",b=>{text+=b;const match=text.match(pattern);if(match){clearTimeout(timer);resolve(match[0])}});child.on("error",reject);child.on("exit",code=>{clearTimeout(timer);reject(new Error(`Exited ${code}: ${text.slice(-600)}`))});});
async function viewer(home){
 const child=spawn(process.env.SLINK_BINARY??"/tmp/session-link-slink",["view","--session",path.join(root,"testdata/viewer/arrival/session.json"),"--no-browser"],{cwd:root,env:{...process.env,SLINK_HOME:home,SLINK_SERVER:base,SLINK_API_KEY:""},stdio:["ignore","pipe","pipe"]});processes.push(child);
 const url=await waitOutput(child,"stdout",/http:\/\/127\.0\.0\.1:\d+\/p\/[\w.-]+/);return {url,origin:new URL(url).origin,home};
}
async function connect(target){
 const socket=new WebSocket(target.webSocketDebuggerUrl);sockets.push(socket);await new Promise((resolve,reject)=>{socket.onopen=resolve;socket.onerror=reject});let seq=0;const pending=new Map();
 socket.onmessage=({data})=>{const m=JSON.parse(data);if(m.id){const call=pending.get(m.id);pending.delete(m.id);m.error?call.reject(new Error(JSON.stringify(m.error))):call.resolve(m.result)}if(m.method==="Runtime.exceptionThrown")errors.push(m.params.exceptionDetails);if(m.method==="Network.requestWillBeSent")requests.push(m.params.request)};
 const send=(method,params={})=>new Promise((resolve,reject)=>{const id=++seq;pending.set(id,{resolve,reject});socket.send(JSON.stringify({id,method,params}))});
 const evaluate=async expression=>{const r=await send("Runtime.evaluate",{expression,awaitPromise:true,returnByValue:true,userGesture:true});if(r.exceptionDetails)throw new Error(JSON.stringify(r.exceptionDetails));return r.result.value};
 const wait=async expression=>{for(let i=0;i<240;i++){if(await evaluate(`!!document.body && (${expression})`))return;await new Promise(r=>setTimeout(r,75))}throw new Error(`Timed out: ${expression}\n${await evaluate("document.body?.innerText")}`)};
 const click=text=>evaluate(`Array.from(document.querySelectorAll('button,a')).find(el=>el.textContent.trim()===${JSON.stringify(text)}).click()`);
 const fill=(selector,value)=>evaluate(`(()=>{const el=document.querySelector(${JSON.stringify(selector)});Object.getOwnPropertyDescriptor(el instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype,'value').set.call(el,${JSON.stringify(value)});el.dispatchEvent(new Event('input',{bubbles:true}));})()`);
 const navigate=async url=>{await send("Page.navigate",{url});};
 await send("Runtime.enable");await send("Network.enable");await send("Page.enable");
 return {send,evaluate,wait,click,fill,navigate};
}
try{
 const a=await viewer(path.join(dir,"device-a"));
 const executable=[process.env.CHROME_PATH,path.join(os.homedir(),"Library/Caches/ms-playwright/chromium_headless_shell-1223/chrome-headless-shell-mac-arm64/chrome-headless-shell"),"/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"].find(p=>p&&existsSync(p));
 const browser=spawn(executable,["--headless=new","--disable-gpu","--no-first-run","--no-default-browser-check","--remote-debugging-port=0",`--user-data-dir=${path.join(dir,"browser")}`,"about:blank"],{stdio:["ignore","ignore","pipe"]});processes.push(browser);
 const endpoint=await waitOutput(browser,"stderr",/ws:\/\/[^\s]+/),debug=`http://127.0.0.1:${new URL(endpoint).port}`;
 const targets=async()=>await(await fetch(debug+"/json/list")).json();
 const page=await connect((await targets()).find(t=>t.type==="page"));
 const newPage=async url=>{const {targetId}=await page.send("Target.createTarget",{url});const p=await connect((await targets()).find(t=>t.id===targetId));return p};
 await page.navigate(a.url+"#span=s8");await page.wait("!!document.querySelector('.sv h1')");
 await page.click("Share this view");await page.wait("document.querySelectorAll('.sv-included-item').length===2");
 await page.evaluate("document.querySelector('.sv-author-fields').open=true");await page.fill('[aria-label="View title"]',"PRIVATE_FIRST_SHARE_TITLE");await page.fill('[aria-label="Your comment"]',"PRIVATE_FIRST_SHARE_NOTE");
 await page.click("Publish link");await page.wait("document.body.innerText.includes('Sign in to continue')");
 const before=await page.evaluate("location.href"),prepared=await page.evaluate("document.querySelector('.sv-saved-notice a').href");
 // Starting/cancelling sign-in keeps the prepared export intact.
 await page.click("Sign in to continue");await page.wait("!!document.querySelector('.sl-auth strong')");await page.click("Cancel sign-in");await page.wait("document.body.innerText.includes('Sign in to continue')");
 await page.click("Sign in to continue");await page.wait("!!document.querySelector('.sl-auth strong')");const code=await page.evaluate("document.querySelector('.sl-auth strong').textContent");
 const authURL=await page.evaluate("document.querySelector('.sl-auth a').href");let target;for(let i=0;i<60;i++){target=(await targets()).find(t=>t.url===authURL);if(target)break;await new Promise(r=>setTimeout(r,100))}assert.ok(target,"sign-in tab opened");
 const auth=await connect(target);await auth.wait("document.body.innerText.includes('Continue to sign in')");await auth.click("Continue to sign in");await auth.wait("!!document.querySelector('input[name=email]')");
 await auth.evaluate("document.querySelector('input[name=email]').value='first-share@example.test';document.querySelector('form').requestSubmit()");await auth.wait("!!document.querySelector('input[name=code]')");
 const mail=await(await fetch(process.env.SLINK_BROWSER_MAIL)).json(),emailCode=mail.text.match(/code is (\d{8})/)[1];
 await auth.evaluate(`document.querySelector('input[name=code]').value=${JSON.stringify(emailCode)};document.querySelector('form').requestSubmit()`);await auth.wait("!!document.querySelector('input[name=user_code]')");
 await auth.evaluate(`document.querySelector('input[name=user_code]').value=${JSON.stringify(code)};document.querySelector('form').requestSubmit()`);
 await page.wait("document.body.innerText.includes('Publish encrypted link')");assert.equal(await page.evaluate("location.href"),before);assert.equal(await page.evaluate("document.querySelector('.sv-saved-notice a').href"),prepared);assert.equal(await page.evaluate("document.querySelector('[aria-label=\"Your comment\"]').value"),"PRIVATE_FIRST_SHARE_NOTE");
 await page.click("Publish encrypted link");await page.wait("document.querySelector('.sl-publish')?.innerText.includes('Published')");const shared=await page.evaluate("document.querySelector('.sl-publish a').href");assert.ok(new URLSearchParams(new URL(shared).hash.slice(1)).get("key"));
 const recipient=await newPage(shared);await recipient.wait("document.body.innerText.includes('PRIVATE_FIRST_SHARE_NOTE')");assert.ok(await recipient.evaluate("document.body.innerText.includes('PRIVATE_FIRST_SHARE_TITLE')"));
 assert.equal((await readFile(path.join(a.home,"previews",new URL(prepared).pathname.split("/").at(-1)+".json"),"utf8")).includes("Earlier recovered failure"),false);
 console.log("✓ First share: cancel/retry, email account creation, in-viewer resume, explicit publish, preserved excerpt and working recipient link");
 // Recovery setup is optional until the user chooses it; it imports this share.
 await page.navigate(a.origin+"/settings");await page.wait("document.body.innerText.includes('Create recovery key')");await page.fill('[aria-label="Device name"]',"Laptop");await page.click("Create recovery key");await page.wait("!!document.querySelector('[data-recovery-key]')");const recoveryKey=await page.evaluate("document.querySelector('[data-recovery-key]').textContent");await page.click("I saved my recovery key");await page.wait("!document.querySelector('[data-recovery-key]')");assert.ok(await page.evaluate("document.body.innerText.includes('1 link in your encrypted vault')"));
 const config=await readFile(path.join(a.home,"config.json"),"utf8");
 const b=await viewer(path.join(dir,"device-b"));await writeFile(path.join(b.home,"config.json"),config,{mode:0o600});const second=await newPage(b.origin+"/settings");await second.wait("document.body.innerText.includes('Request device approval')");assert.equal(await second.evaluate("document.body.innerText.includes('Backed-up share links')"),false);
 await second.fill('[aria-label="Device name"]',"Desktop");await second.click("Request device approval");await second.wait("!!document.querySelector('strong.secret')");const pairing=await second.evaluate("document.querySelector('strong.secret').textContent");
 await page.click("Refresh devices");await page.wait("document.body.innerText.includes('Waiting for approval')");await page.fill('[aria-label="Approval code for Desktop"]',"WRONG");await page.click("Approve device");await page.wait("document.body.innerText.includes('device code does not match')");await page.fill('[aria-label="Approval code for Desktop"]',pairing);await page.click("Approve device");await page.wait("!document.body.innerText.includes('Waiting for approval')");await second.click("Check approval");await second.wait("document.body.innerText.includes('1 link in your encrypted vault')");
 await page.click("Revoke");await page.click("Confirm revocation");await page.wait("!document.body.innerText.includes('Desktop')");await second.click("Refresh devices");await second.wait("document.body.innerText.includes('Request device approval')");
 const c=await viewer(path.join(dir,"device-c"));await writeFile(path.join(c.home,"config.json"),config,{mode:0o600});const third=await newPage(c.origin+"/settings");await third.wait("document.body.innerText.includes('Recover share keys')");await third.fill('input[type=password]',recoveryKey);await third.click("Recover share keys");await third.wait("document.body.innerText.includes('1 link in your encrypted vault')");
 const recoveredLinks=await third.evaluate("Array.from(document.querySelectorAll('a')).map(a=>a.href)");assert.ok(recoveredLinks.includes(shared.split("&span=")[0]),"recovered complete private link");
 for(const request of requests.filter(r=>r.url.startsWith(base))){for(const secret of [recoveryKey,"PRIVATE_FIRST_SHARE_NOTE",new URLSearchParams(new URL(shared).hash.slice(1)).get("key")]){assert.equal(request.url.includes(secret),false);assert.equal(JSON.stringify(request.headers).includes(secret),false);assert.equal((request.postData??"").includes(secret),false)}}
 // Expired/revoked service credentials offer sign-in again in settings.
 await auth.navigate(base+"/account");await auth.wait("!!document.querySelector('form[action=\"/api/account/keys/revoke\"]')");
 await auth.evaluate("document.querySelector('form[action=\"/api/account/keys/revoke\"]').requestSubmit()");
 await auth.wait("!document.querySelector('form[action=\"/api/account/keys/revoke\"]')");
 await page.click("Refresh devices");await page.wait("document.body.innerText.includes('Sign in to continue')");
 await page.send("Emulation.setDeviceMetricsOverride",{width:390,height:844,deviceScaleFactor:1,mobile:true});
 assert.ok(await page.evaluate("document.documentElement.scrollWidth<=innerWidth"));
 assert.deepEqual(errors,[]);const shot=await page.send("Page.captureScreenshot",{format:"png"});await writeFile(path.join(dir,"devices.png"),Buffer.from(shot.data,"base64"));
 console.log(`✓ Recovery setup, account-only lock, code verification, approval, revocation, recovery on a third device and no plaintext in hosted requests (${dir})`);
}finally{for(const socket of sockets)socket.close();for(const child of processes.reverse())child.kill("SIGTERM")}
