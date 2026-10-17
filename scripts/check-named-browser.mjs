#!/usr/bin/env node
// Real named sharing: a native sender, browser recipients and their devices,
// using the smoke server's fake mailbox.
import assert from "node:assert/strict";
import { mkdtemp, writeFile, readFile, readdir } from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
import { launchBrowser, startViewer } from "./browser.mjs";
const root=fileURLToPath(new URL("../",import.meta.url)),base=process.env.SLINK_BROWSER_BASE;
const dir=await mkdtemp(path.join(os.tmpdir(),"slink-named-browser-")),viewers=[];
async function viewer(home){
 const started=await startViewer(["--session",path.join(root,"testdata/viewer/arrival/session.json")],{home,server:base});viewers.push(started.child);return started;
}
async function files(dir) { const out=[];for(const e of await readdir(dir,{withFileTypes:true})){const p=path.join(dir,e.name);out.push(...e.isDirectory()?await files(p):[p])}return out; }
let browser;
try {
 browser=await launchBrowser({profile:path.join(dir,"browser"),gesture:true,ready:"!!document.body",timeout:45_000});
 const {errors,requests}=browser,page=await browser.firstPage();
 const login=async(p,email)=>{
  // Sign-in pages stream their React payload after the form; submitting before the
  // page finishes loading cuts that stream off and React reports "Connection closed".
  await p.waitFor("document.readyState==='complete' && !!document.querySelector('input[name=email]')");
  await p.evaluate(`document.querySelector('input[name=email]').value=${JSON.stringify(email)};document.querySelector('form').requestSubmit()`);
  await p.waitFor("document.readyState==='complete' && !!document.querySelector('input[name=code]')");
  const mail=await(await fetch(process.env.SLINK_BROWSER_MAIL)).json(),code=mail.text.match(/code is (\d{8})/)[1];
  await p.evaluate(`document.querySelector('input[name=code]').value=${JSON.stringify(code)};document.querySelector('form').requestSubmit()`);
 };
 // Native owner signs in through the hosted email/CLI approval flow. Its
 // browser still has no encryption device until approval later in this test.
 await page.navigate(base+"/login?next=/account");await login(page,"named-owner@example.test");await page.waitFor("location.pathname==='/account'");
 const credential=await page.evaluate(`(async()=>{const pending=await(await fetch('/api/auth/cli',{method:'POST'})).json();await fetch('/api/auth/cli/'+pending.code+'/approve',{method:'POST',headers:{'content-type':'application/x-www-form-urlencoded'},body:new URLSearchParams({user_code:pending.user_code})});return(await fetch('/api/auth/cli/'+pending.code)).json()})()`);
 assert.match(credential.key,/^rk_/);
 let a=await viewer(path.join(dir,"owner"));await writeFile(path.join(a.home,"config.json"),JSON.stringify({api_key:credential.key,server:base,user_id:credential.user_id,login:"named-owner@example.test"}),{mode:0o600});
 const local=await browser.newPage(a.url+"&span=s8");await local.waitFor("!!document.querySelector('.sv h1')");await local.click("Share this view");await local.waitFor("document.querySelectorAll('.sv-included-item').length===2");
 await local.evaluate("document.querySelector('.sv-author-fields').open=true");await local.fill('[aria-label="View title"]',"PRIVATE_NAMED_TITLE");await local.fill('[aria-label="Your comment"]',"PRIVATE_NAMED_NOTE");
 await local.click("Publish link");await local.waitFor("!!document.querySelector('select[aria-label]')");
 await local.evaluate("const el=document.querySelector('select[aria-label]');el.value='named';el.dispatchEvent(new Event('change',{bubbles:true}))");
 await local.fill('[aria-label="Recipient emails"]',"named-reader@example.test, second-reader@example.test");await local.waitFor("document.body.innerText.includes('Create recovery key')");
 await local.fill('[aria-label="Device name"]',"Sender laptop");await local.click("Create recovery key");await local.waitFor("!!document.querySelector('[data-recovery-key]')");
 const ownerRecovery=await local.evaluate("document.querySelector('[data-recovery-key]').textContent");await local.click("I saved my recovery key");await local.waitFor("!document.querySelector('[data-recovery-key]')");
 await local.click("Share with these people");await local.waitFor("document.querySelector('.sl-publish')?.innerText.includes('Published for the people')");
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
 const recipient=await browser.newPage(invitation,{isolated:true});await recipient.waitFor("document.body.innerText.includes('Sign in to open this share')");await recipient.click("Sign in to open this share");await login(recipient,"named-reader@example.test");await recipient.waitFor("document.body.innerText.includes('Create recovery key')");
 await recipient.fill('[aria-label="Device name"]',"Reader browser");await recipient.click("Create recovery key");await recipient.waitFor("!!document.querySelector('[data-recovery-key]')");const recovery=await recipient.evaluate("document.querySelector('[data-recovery-key]').textContent");
 // Pending recovery survives reload, then explicit acknowledgment reveals acceptance.
 await recipient.send("Page.reload");await recipient.waitFor("document.body.innerText.includes('Show recovery key')");await recipient.click("Show recovery key");await recipient.waitFor("!!document.querySelector('[data-recovery-key]')");assert.equal(await recipient.evaluate("document.querySelector('[data-recovery-key]').textContent"),recovery);
 await recipient.click("I saved my recovery key");await recipient.waitFor("document.body.innerText.includes('Accept invitation')");await recipient.click("Accept invitation");await recipient.waitFor("document.body.innerText.includes('Waiting for the sender')");
 a=await viewer(a.home);await recipient.waitFor("document.body.innerText.includes('PRIVATE_NAMED_NOTE')");assert.ok(await recipient.evaluate("document.body.innerText.includes('PRIVATE_NAMED_TITLE')"));assert.equal(await recipient.evaluate("location.href"),canonical);
 const remote=await recipient.evaluate(`fetch('/api/named-shares/${id}').then(r=>r.json())`);const firstInbox=JSON.parse(Buffer.from(remote.grant.payload,"base64url")).inbox;
 // A recipient learns nothing about who else was invited.
 const other=outbox.invitations.find(i=>i.email!=="named-reader@example.test"),served=JSON.stringify(remote);
 for(const value of [other.email,other.id,other.commitment])assert.equal(served.includes(value),false,`recipient response leaked ${value}`);
 assert.equal(served.includes("recipients"),false);
 assert.equal(JSON.parse(Buffer.from(remote.statement.payload,"base64url")).invitation.email,"named-reader@example.test");
 const extractable=await recipient.evaluate(`(async()=>{const db=await new Promise((resolve,reject)=>{const r=indexedDB.open("slink-device-v1");r.onsuccess=()=>resolve(r.result);r.onerror=()=>reject(r.error)});const entries=await new Promise(resolve=>{const r=db.transaction("devices").objectStore("devices").getAll();r.onsuccess=()=>resolve(r.result)});db.close();return entries.map(d=>[d.sign.extractable,d.box.extractable])})()`);assert.deepEqual(extractable,[[false,false]]);
 console.log("✓ Named first share: exact excerpt, browser email onboarding, persistent recovery, sender restart and automatic signed grant");
 // A forwarded complete invitation does not grant another account access.
 const wrong=await browser.newPage(invitation,{isolated:true});await wrong.waitFor("document.body.innerText.includes('Sign in to open this share')");await wrong.click("Sign in to open this share");await login(wrong,"wrong-reader@example.test");await wrong.waitFor("document.body.innerText.includes('another account')");
 assert.equal(await wrong.evaluate(`fetch('/api/named-shares/${id}/blob').then(r=>r.status)`),403);
 // A browser belonging to the sender needs approval from the native device.
 await page.navigate(canonical);await page.waitFor("document.body.innerText.includes('Request device approval')");await page.fill('[aria-label="Device name"]',"Sender browser");await page.click("Request device approval");await page.waitFor("!!document.querySelector('strong.secret')");const pairing=await page.evaluate("document.querySelector('strong.secret').textContent");
 await local.navigate(a.origin+"/settings"+new URL(a.url).hash);await local.waitFor("document.body.innerText.includes('Waiting for approval')");await local.fill('[aria-label="Approval code for Sender browser"]',pairing);await local.click("Approve device");await local.waitFor("!document.body.innerText.includes('Waiting for approval')");await page.click("Check approval");await page.waitFor("document.body.innerText.includes('PRIVATE_NAMED_NOTE')");
 // Recovery works entirely in a fresh recipient browser; rotation retains old grants.
 const recovered=await browser.newPage(canonical,{isolated:true});await recovered.waitFor("document.body.innerText.includes('Sign in to open this share')");await recovered.click("Sign in to open this share");await login(recovered,"named-reader@example.test");await recovered.waitFor("document.body.innerText.includes('Recover share keys')");await recovered.fill('[aria-label="Device name"]',"Recovered reader");await recovered.fill('input[type=password]',recovery);await recovered.click("Recover share keys");await recovered.waitFor("document.body.innerText.includes('PRIVATE_NAMED_NOTE')");
 await recovered.navigate(base+"/devices");await recovered.waitFor("document.body.innerText.includes('Reader browser')");await recovered.click("Revoke");await recovered.click("Confirm revocation");await recovered.waitFor("!document.body.innerText.includes('Reader browser')");
 const identity=await recovered.evaluate("fetch('/api/identity').then(r=>r.json())"),newState=JSON.parse(Buffer.from(identity.record.events.at(-1).payload,"base64url"));assert.notEqual(newState.inbox,firstInbox);
 await recovered.navigate(canonical);await recovered.waitFor("document.body.innerText.includes('PRIVATE_NAMED_NOTE')");await recipient.navigate(canonical);await recipient.waitFor("document.body.innerText.includes('Request device approval')");
 // Cookie writes need the app header; an invalid bearer cannot use its cookie.
 assert.equal(await recovered.evaluate("fetch('/api/identity',{method:'POST',headers:{'content-type':'application/json'},body:'{}'}).then(r=>r.status)"),401);
 assert.equal(await recovered.evaluate("fetch('/api/identity',{headers:{authorization:'Bearer invalid'}}).then(r=>r.status)"),401);
 // Revoking the recipient is owner-only and stops all future downloads.
 assert.equal(await recovered.evaluate(`fetch('/api/named-shares/${id}',{method:'POST',headers:{'x-slink':'1','content-type':'application/json'},body:JSON.stringify({action:'revoke',invite_id:${JSON.stringify(remote.invitation.id)}})}).then(r=>r.status)`),403);
 await local.navigate(a.origin+"/shared"+new URL(a.url).hash);await local.waitFor("document.body.innerText.includes('Access granted')");await local.click("Revoke named-reader@example.test");await local.click("Confirm revocation");await local.waitFor("document.body.innerText.includes('Access revoked')");
 assert.equal(await recovered.evaluate(`fetch('/api/named-shares/${id}/blob').then(r=>r.status)`),403);
 const secrets=["PRIVATE_NAMED_NOTE","PRIVATE_NAMED_TITLE",outbox.key,ownerRecovery,recovery,...Object.values(outbox.secrets)];
 for(const request of requests.filter(r=>r.url.startsWith(base))){for(const secret of secrets){assert.equal(request.url.includes(secret),false);assert.equal(JSON.stringify(request.headers).includes(secret),false);assert.equal((request.postData??"").includes(secret),false)}}
 if(process.env.SLINK_BROWSER_DATA)for(const file of await files(process.env.SLINK_BROWSER_DATA)){const bytes=await readFile(file);for(const secret of secrets)assert.equal(bytes.includes(Buffer.from(secret)),false,`private data in ${file}`)}
 await local.send("Emulation.setDeviceMetricsOverride",{width:390,height:844,deviceScaleFactor:1,mobile:true});assert.ok(await local.evaluate("document.documentElement.scrollWidth<=innerWidth"));
 assert.deepEqual(errors,[]);await writeFile(path.join(dir,"named-sharing.png"),await local.screenshot());
 console.log(`✓ Wrong-account denial, native→browser approval, browser recovery, incoming-key rotation, old-grant access, recipient revocation and no plaintext in server requests/storage (${dir})`);
} finally { browser?.close();for(const child of viewers.reverse())child.kill("SIGTERM"); }
