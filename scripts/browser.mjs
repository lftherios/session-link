// A Chromium-family browser driven over the DevTools protocol, and `slink view`
// started for it: what the browser checks and captures in this directory share.
// A module, not a script: it is outside test/ and named so that `node --test`
// does not take it for a test.
//
// The browser is BROWSER_BINARY, else CHROME_PATH, else the newest
// chromium_headless_shell-* in the Playwright cache (~/Library/Caches/ms-playwright
// on macOS, ~/.cache/ms-playwright elsewhere), else Google Chrome or Brave
// where macOS installs them. The captures ask for an installed browser first,
// Brave then Chrome, so that committed images keep the look they were made
// with. A variable that is set is used as it is, so a wrong path fails rather
// than quietly running another browser.
import { spawn } from "node:child_process";
import { existsSync, readdirSync } from "node:fs";
import os from "node:os";
import path from "node:path";
import { fileURLToPath } from "node:url";

const root = fileURLToPath(new URL("../", import.meta.url));
const sleep = (ms) => new Promise((resolve) => setTimeout(resolve, ms));

const FLAGS = [
  "--headless=new", "--disable-gpu", "--no-first-run", "--no-default-browser-check",
  "--disable-background-networking", "--disable-component-update", "--disable-sync",
  "--remote-debugging-port=0",
];

function folders(dir) {
  try {
    return readdirSync(dir, { withFileTypes: true }).filter((entry) => entry.isDirectory()).map((entry) => entry.name);
  } catch {
    return [];
  }
}

// Playwright keeps each headless shell in chromium_headless_shell-<revision>/
// chrome-headless-shell-<platform>/; the highest revision is the newest. The
// cache is in the account's home, which is not HOME when a check is given a
// HOME of its own.
function playwrightShells() {
  let home;
  try {
    home = os.userInfo().homedir;
  } catch {
    home = os.homedir();
  }
  const cache = path.join(home, process.platform === "darwin" ? "Library/Caches/ms-playwright" : ".cache/ms-playwright");
  return folders(cache)
    .filter((name) => /^chromium_headless_shell-\d+$/.test(name))
    .sort((a, b) => b.split("-")[1] - a.split("-")[1])
    .flatMap((name) => folders(path.join(cache, name)).map((platform) => path.join(cache, name, platform, "chrome-headless-shell")));
}

const BRAVE = "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser";
const CHROME = "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome";

export function findBrowser({ installed = false } = {}) {
  const named = process.env.BROWSER_BINARY || process.env.CHROME_PATH;
  if (named) return named;
  const candidates = installed ? [BRAVE, CHROME, ...playwrightShells()] : [...playwrightShells(), CHROME, BRAVE];
  return candidates.find((file) => existsSync(file));
}

// Resolves with the first match of `pattern` in what `child` writes to `stream`.
function waitForOutput(child, stream, pattern, name, timeout = 20_000) {
  return new Promise((resolve, reject) => {
    let text = "";
    const fail = (why) => {
      clearTimeout(timer);
      reject(new Error(`${name} ${why}: ${text.slice(-600)}`));
    };
    const timer = setTimeout(() => fail("did not start in time"), timeout);
    child[stream].on("data", (chunk) => {
      text += chunk;
      const match = text.match(pattern);
      if (match) {
        clearTimeout(timer);
        resolve(match);
      }
    });
    child.once("error", (error) => {
      clearTimeout(timer);
      reject(error);
    });
    child.once("exit", (code, signal) => fail(`exited ${code ?? signal}`));
  });
}

// Runs `slink view <args> --no-browser` with its own home and resolves once it
// prints its URL. SLINK_BINARY names the binary; the server is a closed port
// unless one is given.
export async function startViewer(args, { home, server = "http://127.0.0.1:1", cwd = root, binary = process.env.SLINK_BINARY ?? "/tmp/session-link-slink" }) {
  // A relative SLINK_BINARY names a file from where the script was started.
  const executable = binary.includes(path.sep) ? path.resolve(binary) : binary;
  const child = spawn(executable, ["view", ...args, "--no-browser"], {
    cwd,
    env: { ...process.env, SLINK_HOME: home, SLINK_SERVER: server, SLINK_API_KEY: "" },
    stdio: ["ignore", "pipe", "pipe"],
  });
  let said = "";
  child.stderr.on("data", (chunk) => {
    said += chunk;
  });
  try {
    const [url] = await waitForOutput(child, "stdout", /http:\/\/127\.0\.0\.1:\d+\S*(?=\s)/, "slink view");
    return { child, url, origin: new URL(url).origin, home };
  } catch (error) {
    child.kill();
    if (said) error.message += `\n${said.slice(-600)}`;
    throw error;
  }
}

// Waits until `find` returns something, and returns it.
async function until(find, what) {
  for (let i = 0; i < 100; i++) {
    const found = await find();
    if (found) return found;
    await sleep(50);
  }
  throw new Error(`The browser opened no ${what}`);
}

/**
 * Starts a headless browser with its profile in `profile` and returns a handle
 * to it; `installed` prefers an installed browser, as findBrowser says.
 * `errors` and `requests` collect the exceptions thrown in, and the requests
 * sent from, every page connected through it. The other options are every
 * page's defaults:
 *
 *   gesture   evaluate as a user gesture, so a page can open a window
 *   ready     an expression every waitFor also needs true, such as "!!document.body"
 *   timeout   how long waitFor waits, in milliseconds
 */
export async function launchBrowser({ profile, installed, ...defaults }) {
  const executable = findBrowser({ installed });
  if (!executable) throw new Error("No Chromium-family browser found: set BROWSER_BINARY or CHROME_PATH to one");
  const child = spawn(executable, [...FLAGS, `--user-data-dir=${profile}`, "about:blank"], { stdio: ["ignore", "ignore", "pipe"] });
  let endpoint;
  try {
    [, endpoint] = await waitForOutput(child, "stderr", /DevTools listening on (ws:\/\/\S+)\s/, "the browser");
  } catch (error) {
    child.kill();
    throw error;
  }
  const debug = `http://127.0.0.1:${new URL(endpoint).port}`;
  const sockets = [], errors = [], requests = [];
  let control;

  async function session(url) {
    const socket = new WebSocket(url);
    sockets.push(socket);
    await new Promise((resolve, reject) => {
      socket.onopen = resolve;
      socket.onerror = reject;
    });
    let sequence = 0;
    const pending = new Map();
    socket.onmessage = ({ data }) => {
      const message = JSON.parse(data);
      const call = pending.get(message.id);
      if (call) {
        pending.delete(message.id);
        if (message.error) call.reject(new Error(JSON.stringify(message.error)));
        else call.resolve(message.result);
      } else if (message.method === "Runtime.exceptionThrown") errors.push(message.params.exceptionDetails);
      else if (message.method === "Network.requestWillBeSent") requests.push(message.params.request);
    };
    return (method, params = {}) =>
      new Promise((resolve, reject) => {
        const id = ++sequence;
        pending.set(id, { resolve, reject });
        socket.send(JSON.stringify({ id, method, params }));
      });
  }

  const browser = {
    errors,
    requests,
    targets: async () => (await fetch(`${debug}/json/list`)).json(),
    connect: (target, options) => page(target, { ...defaults, ...options }),
    // The first page can appear a moment after the debugging endpoint does.
    firstPage: async (options) => browser.connect(await until(async () => (await browser.targets()).find((target) => target.type === "page"), "page"), options),
    // A new tab, in a context of its own when `isolated`: no cookies or storage shared.
    newPage: async (url, { isolated = false, ...options } = {}) => {
      control ??= session(endpoint);
      const send = await control;
      const context = isolated ? await send("Target.createBrowserContext") : {};
      const { targetId } = await send("Target.createTarget", { url, ...context });
      return browser.connect(await until(async () => (await browser.targets()).find((target) => target.id === targetId), "new page"), options);
    },
    close() {
      for (const socket of sockets) socket.close();
      child.kill();
    },
  };

  async function page(target, { gesture = false, ready = "", timeout = 10_000 }) {
    const send = await session(target.webSocketDebuggerUrl);
    const evaluate = async (expression) => {
      const result = await send("Runtime.evaluate", { expression, awaitPromise: true, returnByValue: true, userGesture: gesture });
      if (result.exceptionDetails) throw new Error(JSON.stringify(result.exceptionDetails));
      return result.result.value;
    };
    // A page that is still arriving can have no body yet, and the browser drops
    // a question asked across a navigation. Neither is an answer, so ask again.
    const waitFor = async (expression) => {
      const check = ready ? `(${ready}) && (${expression})` : expression;
      const end = Date.now() + timeout;
      let last;
      while (true) {
        try {
          if (await evaluate(check)) return;
          last = undefined;
        } catch (error) {
          last = error;
        }
        if (Date.now() > end) break;
        await sleep(50);
      }
      const seen = await evaluate("location.href + '\\n' + (document.body?.innerText ?? '').slice(0, 1000)").catch(() => "");
      throw new Error(`Timed out: ${expression}\n${last ? `last error: ${last.message}\n` : ""}page: ${seen}`);
    };
    // Clicks the first button, link or summary whose text is `text`.
    const click = (text) =>
      evaluate(`(() => {
        const el = Array.from(document.querySelectorAll('button,a,summary')).find((el) => el.textContent.trim() === ${JSON.stringify(text)});
        if (!el) throw new Error(${JSON.stringify(`Nothing to click reads ${JSON.stringify(text)}`)});
        el.click();
      })()`);
    // Sets a field's value as typing would, so React sees the change.
    const fill = (selector, value) =>
      evaluate(`(() => {
        const el = document.querySelector(${JSON.stringify(selector)});
        const proto = el instanceof HTMLTextAreaElement ? HTMLTextAreaElement.prototype : HTMLInputElement.prototype;
        Object.getOwnPropertyDescriptor(proto, 'value').set.call(el, ${JSON.stringify(value)});
        el.dispatchEvent(new Event('input', { bubbles: true }));
      })()`);
    const navigate = (url) => send("Page.navigate", { url });
    const screenshot = async (params = { format: "png" }) => Buffer.from((await send("Page.captureScreenshot", params)).data, "base64");
    await send("Runtime.enable");
    await send("Network.enable");
    await send("Page.enable");
    return { send, evaluate, waitFor, click, fill, navigate, screenshot };
  }

  return browser;
}
