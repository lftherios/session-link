#!/usr/bin/env node
// Invoked by the hosted service's smoke-encrypted.mjs against its fixture server.
import assert from "node:assert/strict";
import { mkdtemp, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { launchBrowser } from "./browser.mjs";

const base = process.env.SLINK_BROWSER_BASE, link = process.env.SLINK_BROWSER_LINK;
if (!base || !link || !process.env.SLINK_BROWSER_MAIL) throw new Error("Run through smoke-encrypted.mjs with SLINK_BROWSER_SCRIPT set to this file");
const contentKey = new URLSearchParams(new URL(link).hash.slice(1)).get("key");
const dir = await mkdtemp(path.join(os.tmpdir(), "slink-encrypted-browser-"));
const browser = await launchBrowser({ profile: dir, ready: "!!document.body" });
try {
  const page = await browser.firstPage(), { errors, requests } = browser;
  const { send, evaluate, waitFor, navigate } = page;
  const screenshot = async name => writeFile(path.join(dir, `${name}.png`), await page.screenshot());
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
  // Sign-in pages stream their React payload after the form; submitting before the
  // page finishes loading cuts that stream off and React reports "Connection closed".
  await navigate(base+"/login");await waitFor("document.readyState==='complete' && !!document.querySelector('input[name=email]')");
  await evaluate("document.querySelector('input[name=email]').value='browser@example.test';document.querySelector('form').requestSubmit()");
  await waitFor("document.readyState==='complete' && !!document.querySelector('input[name=code]')");
  let email=await (await fetch(process.env.SLINK_BROWSER_MAIL)).json();
  const code=email.text.match(/code is (\d{8})/)[1];
  await evaluate(`document.querySelector('input[name=code]').value=${JSON.stringify(code)};document.querySelector('form').requestSubmit()`);
  await waitFor("location.pathname==='/account' && document.body.innerText.includes('browser@example.test')");
  await screenshot("email-account");
  await evaluate("document.querySelector('form[action=\"/api/auth/logout\"]').requestSubmit()");
  await waitFor("document.readyState==='complete' && !!document.querySelector('input[name=email]')");
  await evaluate("document.querySelector('input[name=email]').value='browser@example.test';document.querySelector('form').requestSubmit()");
  await waitFor("document.readyState==='complete' && !!document.querySelector('input[name=code]')");
  email=await (await fetch(process.env.SLINK_BROWSER_MAIL)).json();
  const magicLink=email.text.match(/http:\/\/[^\s]+#token=[\w-]+/)[0];
  await navigate(magicLink);await waitFor("document.readyState==='complete' && !!document.querySelector('input[name=token]')");
  assert.match(await evaluate("location.pathname"),/^\/login\/email\//,"GET consumed magic link");
  await evaluate("document.querySelector('form').requestSubmit()");
  await waitFor("location.pathname==='/account' && document.body.innerText.includes('browser@example.test')");
  assert.deepEqual(errors,[]);
  console.log(`✓ Browser decryption, fragment-key changes, missing/wrong keys, email code and magic-link confirmation (${dir})`);
} finally { browser.close(); }
