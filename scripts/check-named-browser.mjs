#!/usr/bin/env node
// Real first-share + recovery/device UI, using the smoke server's fake mailbox.
import assert from "node:assert/strict";
import { spawn } from "node:child_process";
import { existsSync } from "node:fs";
import { mkdtemp, writeFile, readFile, readdir } from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
const root=fileURLToPath(new URL("../",import.meta.url)),base=process.env.SLINK_BROWSER_BASE;
const dir=await mkdtemp(path.join(os.tmpdir(),"slink-named-browser-")),processes=[],sockets=[],errors=[],requests=[];
const waitOutput=(child,stream,pattern)=>new Promise((resolve,reject)=>{let text="";const timer=setTimeout(()=>reject(new Error(`Startup timed out: ${text.slice(-600)}`)),20000);child[stream].on("data",b=>{text+=b;const match=text.match(pattern);if(match){clearTimeout(timer);resolve(match[0])}});child.on("error",reject);child.on("exit",code=>{clearTimeout(timer);reject(new Error(`Exited ${code}: ${text.slice(-600)}`))});});
async function viewer(home){
 const child=spawn(process.env.SLINK_BINARY??"/tmp/session-link-slink",["view","--session",path.join(root,"testdata/viewer/arrival/session.json"),"--no-browser"],{cwd:root,env:{...process.env,SLINK_HOME:home,SLINK_SERVER:base,SLINK_API_KEY:""},stdio:["ignore","pipe","pipe"]});processes.push(child);
 const url=await waitOutput(child,"stdout",/http:\/\/127\.0\.0\.1:\d+\/p\/[^\s]+/);return {url,origin:new URL(url).origin,home,child};
}
async function connect(target,browser=false){
 const socket=new WebSocket(target.webSocketDebuggerUrl);sockets.push(socket);await new Promise((resolve,reject)=>{socket.onopen=resolve;socket.onerror=reject});let seq=0;const pending=new Map();
 socket.onmessage=({data})=>{const m=JSON.parse(data);if(m.id){const call=pending.get(m.id);pending.delete(m.id);m.error?call.reject(new Error(JSON.stringify(m.error))):call.resolve(m.result)}if(m.method==="Runtime.exceptionThrown")errors.push(m.params.exceptionDetails);if(m.method==="Network.requestWillBeSent")requests.push(m.params.request)};
 const send=(method,params={})=>new Promise((resolve,reject)=>{const id=++seq;pending.set(id,{resolve,reject});socket.send(JSON.stringify({id,method,params}))});
 const evaluate=async expression=>{const r=await send("Runtime.evaluate",{expression,awaitPromise:true,returnByValue:true,userGesture:true});if(r.exceptionDetails)throw new Error(JSON.stringify(r.exceptionDetails));return r.result.value};
 const wait=async expression=>{for(let i=0;i<600;i++){if(await evaluate(`!!document.body && (${expression})`))return;await new Promise(r=>setTimeout(r,75))}throw new Error(`Timed out: ${expression}\n${await evaluate("document.body?.innerText")}`)};
 const click=text=>evaluate(`Array.from(document.querySelectorAll('button,a')).find(el=>el.textContent.trim()===${JSON.stringify(text)}).click()`);
 const fill=(selector,value)=>evaluate(`(()=>{const el=document.querySelector(${JSON.stringify(selector)});Object.getOwnPropertyDescriptor(el instanceof HTMLTextAreaElement?HTMLTextAreaElement.prototype:HTMLInputElement.prototype,'value').set.call(el,${JSON.stringify(value)});el.dispatchEvent(new Event('input',{bubbles:true}));})()`);
 const navigate=async url=>{await send("Page.navigate",{url});};
 if(!browser){await send("Runtime.enable");await send("Network.enable");await send("Page.enable");}
 return {send,evaluate,wait,click,fill,navigate};
}
async function files(dir) { const out=[];for(const e of await readdir(dir,{withFileTypes:true})){const p=path.join(dir,e.name);out.push(...e.isDirectory()?await files(p):[p])}return out; }
try {
 const executable=[process.env.CHROME_PATH,path.join(os.homedir(),"Library/Caches/ms-playwright/chromium_headless_shell-1223/chrome-headless-shell-mac-arm64/chrome-headless-shell"),"/Applications/Brave Browser.app/Contents/MacOS/Brave Browser"].find(p=>p&&existsSync(p));
 const browser=spawn(executable,["--headless=new","--disable-gpu","--no-first-run","--no-default-browser-check","--remote-debugging-port=0",`--user-data-dir=${path.join(dir,"browser")}`,"about:blank"],{stdio:["ignore","ignore","pipe"]});processes.push(browser);
 const endpoint=await waitOutput(browser,"stderr",/ws:\/\/[^\s]+/),debug=`http://127.0.0.1:${new URL(endpoint).port}`;
 const targets=async()=>await(await fetch(debug+"/json/list")).json();
 // The first page can appear a moment after the debugging endpoint does.
 const firstPage=async()=>{for(let i=0;i<100;i++){const t=(await targets()).find(t=>t.type==="page");if(t)return t;await new Promise(r=>setTimeout(r,50))}throw new Error("The browser opened no page")};
 const page=await connect(await firstPage());
 const controller=await connect({webSocketDebuggerUrl:endpoint},true);
 const newPage=async(url,isolated=false)=>{const context=isolated?await controller.send("Target.createBrowserContext"):{};const {targetId}=await controller.send("Target.createTarget",{url,...context});return connect((await targets()).find(t=>t.id===targetId));};
 const login=async(p,email)=>{
  // Sign-in pages stream their React payload after the form; submitting before the
  // page finishes loading cuts that stream off and React reports "Connection closed".
  await p.wait("document.readyState==='complete' && !!document.querySelector('input[name=email]')");
  await p.evaluate(`document.querySelector('input[name=email]').value=${JSON.stringify(email)};document.querySelector('form').requestSubmit()`);
  await p.wait("document.readyState==='complete' && !!document.querySelector('input[name=code]')");
  const mail=await(await fetch(process.env.SLINK_BROWSER_MAIL)).json(),code=mail.text.match(/code is (\d{8})/)[1];
  await p.evaluate(`document.querySelector('input[name=code]').value=${JSON.stringify(code)};document.querySelector('form').requestSubmit()`);
 };
 // Native owner signs in through the hosted email/CLI approval flow. Its
 // browser still has no encryption device until approval later in this test.
 await page.navigate(base+"/login?next=/account");await login(page,"named-owner@example.test");await page.wait("location.pathname==='/account'");
 const credential=await page.evaluate(`(async()=>{const pending=await(await fetch('/api/auth/cli',{method:'POST'})).json();await fetch('/api/auth/cli/'+pending.code+'/approve',{method:'POST',headers:{'content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams({user_code:pending.user_code})});return(await fetch('/api/auth/cli/'+pending.code)).json()})()`);
 assert.match(credential.key,/^rk_/);
 let a=await viewer(path.join(dir,"owner"));await writeFile(path.join(a.home,"config.json"),JSON.stringify({api_key:credential.key,server:base,user_id:credential.user_id,login:"named-owner@example.test"}),{mode:0o600});
 const local=await newPage(a.url+"&span=s8");await local.wait("!!document.querySelector('.sv h1')");await local.click("Share this view");await local.wait("document.querySelectorAll('.sv-included-item').length===2");
 await local.evaluate("document.querySelector('.sv-author-fields').open=true");await local.fill('[aria-label="View title"]',"PRIVATE_NAMED_TITLE");await local.fill('[aria-label="Your comment"]',"PRIVATE_NAMED_NOTE");
 await local.click("Publish link");await local.wait("!!document.querySelector('select[aria-label]')");
 await local.evaluate("const el=document.querySelector('select[aria-label]');el.value='named';el.dispatchEvent(new Event('change',{bubbles:true}))");
 await local.fill('[aria-label="Recipient emails"]',"named-reader@example.test, second-reader@example.test");await local.wait("document.body.innerText.includes('Create recovery key')");
 await local.fill('[aria-label="Device name"]',"Sender laptop");await local.click("Create recovery key");await local.wait("!!document.querySelector('[data-recovery-key]')");
 const ownerRecovery=await local.evaluate("document.querySelector('[data-recovery-key]').textContent");await local.click("I saved my recovery key");await local.wait("!document.querySelector('[data-recovery-key]')");
 await local.click("Share with these people");await local.wait("document.querySelector('.sl-publish')?.innerText.includes('Published for the people')");
 const prepared=await local.evaluate("document.querySelector('.sv-saved-notice a').href");
 const invitation=await local.evaluate("Array.from(document.querySelectorAll('.sl-publish li')).find(li=>li.querySelector('strong')?.textContent==='named-reader@example.test').querySelector('a').href"),id=new URL(invitation).pathname.split('/').at(-1),canonical=base+"/n/"+id;
 const outbox=JSON.parse(await readFile(path.join(a.home,"named-shares",(await readdir(path.join(a.home,"named-shares"))).find(f=>!f.startsWith('pending-'))),"utf8"));
 assert.equal(new URL(invitation).hash.includes(outbox.key),false);
 const ciphertext=Buffer.from(await(await fetch(base+"/api/named-shares/"+id+"/blob",{headers:{authorization:"Bearer "+credential.key}})).arrayBuffer());
 const contentKey=await crypto.subtle.importKey("raw",Buffer.from(outbox.key,"base64url"),"AES-GCM",false,["decrypt"]);
 const plaintext=Buffer.from(await crypto.subtle.decrypt({name:"AES-GCM",iv:ciphertext.subarray(12,24),additionalData:ciphertext.subarray(0,24)},contentKey,ciphertext.subarray(24)));
 const preparedBytes=await readFile(path.join(a.home,"previews",new URL(prepared).pathname.split("/").at(-1)+".json"));assert.deepEqual(plaintext,preparedBytes);assert.equal(plaintext.includes(Buffer.from("Earlier recovered failure")),false);
 assert.equal((await fetch(base+"/api/shares/"+id)).status,404);assert.equal((await fetch(base+"/api/named-shares/"+id+"/blob")).status,401);
 a.child.kill("SIGTERM");
 const recipient=await newPage(invitation,true);await recipient.wait("document.body.innerText.includes('Sign in to open this share')");await recipient.click("Sign in to open this share");await login(recipient,"named-reader@example.test");await recipient.wait("document.body.innerText.includes('Create recovery key')");
 await recipient.fill('[aria-label="Device name"]',"Reader browser");await recipient.click("Create recovery key");await recipient.wait("!!document.querySelector('[data-recovery-key]')");const recovery=await recipient.evaluate("document.querySelector('[data-recovery-key]').textContent");
 // Pending recovery survives reload, then explicit acknowledgment reveals acceptance.
 await recipient.send("Page.reload");await recipient.wait("document.body.innerText.includes('Show recovery key')");await recipient.click("Show recovery key");await recipient.wait("!!document.querySelector('[data-recovery-key]')");assert.equal(await recipient.evaluate("document.querySelector('[data-recovery-key]').textContent"),recovery);
 await recipient.click("I saved my recovery key");await recipient.wait("document.body.innerText.includes('Accept invitation')");await recipient.click("Accept invitation");await recipient.wait("document.body.innerText.includes('Waiting for the sender')");
 a=await viewer(a.home);await recipient.wait("document.body.innerText.includes('PRIVATE_NAMED_NOTE')");assert.ok(await recipient.evaluate("document.body.innerText.includes('PRIVATE_NAMED_TITLE')"));assert.equal(await recipient.evaluate("location.href"),canonical);
 const remote=await recipient.evaluate(`fetch('/api/named-shares/${id}').then(r=>r.json())`);const firstInbox=JSON.parse(Buffer.from(remote.grant.payload,"base64url")).inbox;
 // A recipient learns nothing about who else was invited.
 const other=outbox.invitations.find(i=>i.email!=="named-reader@example.test"),served=JSON.stringify(remote);
 for(const value of [other.email,other.id,other.commitment])assert.equal(served.includes(value),false,`recipient response leaked ${value}`);
 assert.equal(served.includes("recipients"),false);
 assert.equal(JSON.parse(Buffer.from(remote.statement.payload,"base64url")).invitation.email,"named-reader@example.test");
 const extractable=await recipient.evaluate(`(async()=>{const db=await new Promise((resolve,reject)=>{const r=indexedDB.open("slink-device-v1");r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});const entries=await new Promise(resolve=>{const r=db.transaction("devices").objectStore("devices").getAll();r.onsuccess=()=>resolve(r.result)});db.close();return entries.map(d=>[d.sign.extractable,d.box.extractable])})()`);assert.deepEqual(extractable,[[false,false]]);
 console.log("✓ Named first share: exact excerpt, browser email onboarding, persistent recovery, sender restart and automatic signed grant");
 // A forwarded complete invitation does not grant another account access.
 const wrong=await newPage(invitation,true);await wrong.wait("document.body.innerText.includes('Sign in to open this share')");await wrong.click("Sign in to open this share");await login(wrong,"wrong-reader@example.test");await wrong.wait("document.body.innerText.includes('another account')");
 assert.equal(await wrong.evaluate(`fetch('/api/named-shares/${id}/blob').then(r=>r.status)`),403);
 // A browser belonging to the sender needs approval from the native device.
 await page.navigate(canonical);await page.wait("document.body.innerText.includes('Request device approval')");await page.fill('[aria-label="Device name"]',"Sender browser");await page.click("Request device approval");await page.wait("!!document.querySelector('strong.secret')");const pairing=await page.evaluate("document.querySelector('strong.secret').textContent");
 await local.navigate(a.origin+"/settings"+new URL(a.url).hash);await local.wait("document.body.innerText.includes('Waiting for approval')");await local.fill('[aria-label="Approval code for Sender browser"]',pairing);await local.click("Approve device");await local.wait("!document.body.innerText.includes('Waiting for approval')");await page.click("Check approval");await page.wait("document.body.innerText.includes('PRIVATE_NAMED_NOTE')");
 // Recovery works entirely in a fresh recipient browser; rotation retains old grants.
 const recovered=await newPage(canonical,true);await recovered.wait("document.body.innerText.includes('Sign in to open this share')");await recovered.click("Sign in to open this share");await login(recovered,"named-reader@example.test");await recovered.wait("document.body.innerText.includes('Recover share keys')");await recovered.fill('[aria-label="Device name"]',"Recovered reader");await recovered.fill('input[type=password]',recovery);await recovered.click("Recover share keys");await recovered.wait("document.body.innerText.includes('PRIVATE_NAMED_NOTE')");
 await recovered.navigate(base+"/devices");await recovered.wait("document.body.innerText.includes('Reader browser')");await recovered.click("Revoke");await recovered.click("Confirm revocation");await recovered.wait("!document.body.innerText.includes('Reader browser')");
 const identity=await recovered.evaluate("fetch('/api/identity').then(r=>r.json())"),newState=JSON.parse(Buffer.from(identity.record.events.at(-1).payload,"base64url"));assert.notEqual(newState.inbox,firstInbox);
 await recovered.navigate(canonical);await recovered.wait("document.body.innerText.includes('PRIVATE_NAMED_NOTE')");await recipient.navigate(canonical);await recipient.wait("document.body.innerText.includes('Request device approval')");
 // Cookie writes need the app header; an invalid bearer cannot use its cookie.
 assert.equal(await recovered.evaluate("fetch('/api/identity',{method:'POST',headers:{'content-type':'application/json'},body:'{}'}).then(r=>r.status)"),401);
 assert.equal(await recovered.evaluate("fetch('/api/identity',{headers:{authorization:'Bearer invalid'}}).then(r=>r.status)"),401);
 // Revoking the recipient is owner-only and stops all future downloads.
 assert.equal(await recovered.evaluate(`fetch('/api/named-shares/${id}',{method:'POST',headers:{'x-slink':'1','content-type':'application/json'},body:JSON.stringify({action:'revoke',invite_id:${JSON.stringify(remote.invitation.id)}})}).then(r=>r.status)`),403);
 await local.navigate(a.origin+"/shared"+new URL(a.url).hash);await local.wait("document.body.innerText.includes('Access granted')");await local.click("Revoke named-reader@example.test");await local.click("Confirm revocation");await local.wait("document.body.innerText.includes('Access revoked')");
 assert.equal(await recovered.evaluate(`fetch('/api/named-shares/${id}/blob').then(r=>r.status)`),403);
 const secrets=["PRIVATE_NAMED_NOTE","PRIVATE_NAMED_TITLE",outbox.key,ownerRecovery,recovery,...Object.values(outbox.secrets)];
 for(const request of requests.filter(r=>r.url.startsWith(base))){for(const secret of secrets){assert.equal(request.url.includes(secret),false);assert.equal(JSON.stringify(request.headers).includes(secret),false);assert.equal((request.postData??"").includes(secret),false)}}
 if(process.env.SLINK_BROWSER_DATA)for(const file of await files(process.env.SLINK_BROWSER_DATA)){const bytes=await readFile(file);for(const secret of secrets)assert.equal(bytes.includes(Buffer.from(secret)),false,`private data in ${file}`)}
 await local.send("Emulation.setDeviceMetricsOverride",{width:390,height:844,deviceScaleFactor:1,mobile:true});assert.ok(await local.evaluate("document.documentElement.scrollWidth<=innerWidth"));
 assert.deepEqual(errors,[]);const shot=await local.send("Page.captureScreenshot",{format:"png"});await writeFile(path.join(dir,"named-sharing.png"),Buffer.from(shot.data,"base64"));
 console.log(`✓ Wrong-account denial, native→browser approval, browser recovery, incoming-key rotation, old-grant access, recipient revocation and no plaintext in server requests/storage (${dir})`);
} finally { for(const socket of sockets)socket.close();for(const child of processes.reverse())child.kill("SIGTERM"); }
