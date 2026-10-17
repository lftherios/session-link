#!/usr/bin/env node
// Real first-share + recovery/device UI, using the smoke server's fake mailbox.
import "./check-encrypted-browser.mjs";
import assert from "node:assert/strict";
import { mkdtemp, writeFile, readFile } from "node:fs/promises";
import path from "node:path";
import os from "node:os";
import { fileURLToPath } from "node:url";
import { launchBrowser, startViewer } from "./browser.mjs";
const root=fileURLToPath(new URL("../",import.meta.url)),base=process.env.SLINK_BROWSER_BASE;
const dir=await mkdtemp(path.join(os.tmpdir(),"slink-identity-browser-")),viewers=[];
async function viewer(home){
 const started=await startViewer(["--session",path.join(root,"testdata/viewer/arrival/session.json")],{home,server:base});viewers.push(started.child);return started;
}
let browser;
try{
 const a=await viewer(path.join(dir,"device-a"));
 browser=await launchBrowser({profile:path.join(dir,"browser"),gesture:true,ready:"!!document.body",timeout:18_000});
 const {errors,requests}=browser,page=await browser.firstPage();
 await page.navigate(a.url+"&span=s8");await page.waitFor("!!document.querySelector('.sv h1')");
 await page.click("Share this view");await page.waitFor("document.querySelectorAll('.sv-included-item').length===2");
 await page.evaluate("document.querySelector('.sv-author-fields').open=true");await page.fill('[aria-label="View title"]',"PRIVATE_FIRST_SHARE_TITLE");await page.fill('[aria-label="Your comment"]',"PRIVATE_FIRST_SHARE_NOTE");
 await page.click("Publish link");await page.waitFor("document.body.innerText.includes('Sign in to continue')");
 const before=await page.evaluate("location.href"),prepared=await page.evaluate("document.querySelector('.sv-saved-notice a').href");
 // Starting/cancelling sign-in keeps the prepared export intact.
 await page.click("Sign in to continue");await page.waitFor("!!document.querySelector('.sl-auth strong')");await page.click("Cancel sign-in");await page.waitFor("document.body.innerText.includes('Sign in to continue')");
 await page.click("Sign in to continue");await page.waitFor("!!document.querySelector('.sl-auth strong')");const code=await page.evaluate("document.querySelector('.sl-auth strong').textContent");
 const authURL=await page.evaluate("document.querySelector('.sl-auth a').href");let target;for(let i=0;i<60;i++){target=(await browser.targets()).find(t=>t.url===authURL);if(target)break;await new Promise(r=>setTimeout(r,100))}assert.ok(target,"sign-in tab opened");
 // Sign-in pages stream their React payload after the form; submitting before the
 // page finishes loading cuts that stream off and React reports "Connection closed".
 const auth=await browser.connect(target);await auth.waitFor("document.body.innerText.includes('Continue to sign in')");await auth.click("Continue to sign in");await auth.waitFor("document.readyState==='complete' && !!document.querySelector('input[name=email]')");
 await auth.evaluate("document.querySelector('input[name=email]').value='first-share@example.test';document.querySelector('form').requestSubmit()");await auth.waitFor("document.readyState==='complete' && !!document.querySelector('input[name=code]')");
 const mail=await(await fetch(process.env.SLINK_BROWSER_MAIL)).json(),emailCode=mail.text.match(/code is (\d{8})/)[1];
 await auth.evaluate(`document.querySelector('input[name=code]').value=${JSON.stringify(emailCode)};document.querySelector('form').requestSubmit()`);await auth.waitFor("document.readyState==='complete' && !!document.querySelector('input[name=user_code]')");
 await auth.evaluate(`document.querySelector('input[name=user_code]').value=${JSON.stringify(code)};document.querySelector('form').requestSubmit()`);
 await page.waitFor("document.body.innerText.includes('Publish encrypted link')");assert.equal(await page.evaluate("location.href"),before);assert.equal(await page.evaluate("document.querySelector('.sv-saved-notice a').href"),prepared);assert.equal(await page.evaluate("document.querySelector('[aria-label=\"Your comment\"]').value"),"PRIVATE_FIRST_SHARE_NOTE");
 await page.click("Publish encrypted link");await page.waitFor("document.querySelector('.sl-publish')?.innerText.includes('Published')");const shared=await page.evaluate("document.querySelector('.sl-publish a').href");assert.ok(new URLSearchParams(new URL(shared).hash.slice(1)).get("key"));
 const recipient=await browser.newPage(shared);await recipient.waitFor("document.body.innerText.includes('PRIVATE_FIRST_SHARE_NOTE')");assert.ok(await recipient.evaluate("document.body.innerText.includes('PRIVATE_FIRST_SHARE_TITLE')"));
 assert.equal((await readFile(path.join(a.home,"previews",new URL(prepared).pathname.split("/").at(-1)+".json"),"utf8")).includes("Earlier recovered failure"),false);
 console.log("✓ First share: cancel/retry, email account creation, in-viewer resume, explicit publish, preserved excerpt and working recipient link");
 // Recovery setup is optional until the user chooses it; it imports this share.
 await page.navigate(a.origin+"/settings");await page.waitFor("document.body.innerText.includes('Create recovery key')");await page.fill('[aria-label="Device name"]',"Laptop");await page.click("Create recovery key");await page.waitFor("!!document.querySelector('[data-recovery-key]')");const recoveryKey=await page.evaluate("document.querySelector('[data-recovery-key]').textContent");await page.click("I saved my recovery key");await page.waitFor("!document.querySelector('[data-recovery-key]')");assert.ok(await page.evaluate("document.body.innerText.includes('1 link in your encrypted vault')"));
 const config=await readFile(path.join(a.home,"config.json"),"utf8");
 const b=await viewer(path.join(dir,"device-b"));await writeFile(path.join(b.home,"config.json"),config,{mode:0o600});const second=await browser.newPage(b.origin+"/settings"+new URL(b.url).hash);await second.waitFor("document.body.innerText.includes('Request device approval')");assert.equal(await second.evaluate("document.body.innerText.includes('Backed-up share links')"),false);
 await second.fill('[aria-label="Device name"]',"Desktop");await second.click("Request device approval");await second.waitFor("!!document.querySelector('strong.secret')");const pairing=await second.evaluate("document.querySelector('strong.secret').textContent");
 await page.click("Refresh devices");await page.waitFor("document.body.innerText.includes('Waiting for approval')");await page.fill('[aria-label="Approval code for Desktop"]',"WRONG");await page.click("Approve device");await page.waitFor("document.body.innerText.includes('device code does not match')");await page.fill('[aria-label="Approval code for Desktop"]',pairing);await page.click("Approve device");await page.waitFor("!document.body.innerText.includes('Waiting for approval')");await second.click("Check approval");await second.waitFor("document.body.innerText.includes('1 link in your encrypted vault')");
 await page.click("Revoke");await page.click("Confirm revocation");await page.waitFor("!document.body.innerText.includes('Desktop')");await second.click("Refresh devices");await second.waitFor("document.body.innerText.includes('Request device approval')");
 const c=await viewer(path.join(dir,"device-c"));await writeFile(path.join(c.home,"config.json"),config,{mode:0o600});const third=await browser.newPage(c.origin+"/settings"+new URL(c.url).hash);await third.waitFor("document.body.innerText.includes('Recover share keys')");await third.fill('input[type=password]',recoveryKey);await third.click("Recover share keys");await third.waitFor("document.body.innerText.includes('1 link in your encrypted vault')");
 const recoveredLinks=await third.evaluate("Array.from(document.querySelectorAll('a')).map(a=>a.href)");assert.ok(recoveredLinks.includes(shared.split("&span=")[0]),"recovered complete private link");
 for(const request of requests.filter(r=>r.url.startsWith(base))){for(const secret of [recoveryKey,"PRIVATE_FIRST_SHARE_NOTE",new URLSearchParams(new URL(shared).hash.slice(1)).get("key")]){assert.equal(request.url.includes(secret),false);assert.equal(JSON.stringify(request.headers).includes(secret),false);assert.equal((request.postData??"").includes(secret),false)}}
 // Expired/revoked service credentials offer sign-in again in settings.
 await auth.navigate(base+"/account");await auth.waitFor("!!document.querySelector('form[action=\"/api/account/keys/revoke\"]')");
 await auth.evaluate("document.querySelector('form[action=\"/api/account/keys/revoke\"]').requestSubmit()");
 await auth.waitFor("!document.querySelector('form[action=\"/api/account/keys/revoke\"]')");
 await page.click("Refresh devices");await page.waitFor("document.body.innerText.includes('Sign in to continue')");
 await page.send("Emulation.setDeviceMetricsOverride",{width:390,height:844,deviceScaleFactor:1,mobile:true});
 assert.ok(await page.evaluate("document.documentElement.scrollWidth<=innerWidth"));
 assert.deepEqual(errors,[]);await writeFile(path.join(dir,"devices.png"),await page.screenshot());
 console.log(`✓ Recovery setup, account-only lock, code verification, approval, revocation, recovery on a third device and no plaintext in hosted requests (${dir})`);
}finally{browser?.close();for(const child of viewers.reverse())child.kill("SIGTERM")}
