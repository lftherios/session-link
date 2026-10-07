// Only the local viewer loads this script. The hosted viewer never gets a
// local capability, and cookies never carry it to other localhost ports.
(() => {
  const bootstrap = document.currentScript.hasAttribute("data-bootstrap");
  // Pasting the complete URL into a locked page only changes its fragment.
  // Retry that navigation; document.open removes this shell's listeners.
  if (bootstrap) window.addEventListener("hashchange", () => location.reload(), { once: true });
  const fragment = new URLSearchParams(location.hash.slice(1));
  const supplied = fragment.get("access");
  let token = supplied;
  try { token ??= sessionStorage.getItem("slink-local-access"); } catch {}
  if (!token || !/^[A-Za-z0-9_-]{43}$/.test(token)) {
    if (bootstrap) document.getElementById("access-status").textContent = "Open the complete local URL printed by slink view.";
    return;
  }
  try { sessionStorage.setItem("slink-local-access", token); } catch {}
  fragment.set("access", token);
  history.replaceState(null, "", location.pathname + location.search + "#" + fragment);

  const nativeFetch = window.__slinkNativeFetch ??= window.fetch.bind(window);
  window.fetch = (input, init) => {
    const request = new Request(input instanceof Request ? input : new URL(input, location.href), init);
    if (new URL(request.url).origin !== location.origin) return nativeFetch(request);
    const headers = new Headers(request.headers);
    headers.set("x-slink-access", token);
    // A redirect must not forward the capability to another origin.
    return nativeFetch(new Request(request, { headers, redirect: "error" }));
  };

  if (bootstrap) {
    fetch(location.pathname + location.search).then(async response => {
      if (!response.ok) throw new Error("This local link has expired. Open the URL from the running slink viewer.");
      const html = await response.text();
      document.open(); document.write(html); document.close();
    }).catch(error => { document.getElementById("access-status").textContent = error.message; });
    return;
  }

  // Carry access across new tabs, copied links, and navigation without relying
  // on opener access or on sessionStorage being shared between tabs.
  const decorate = () => {
    for (const link of document.querySelectorAll("a[href]")) {
      const url = new URL(link.getAttribute("href"), location.href);
      if (url.origin !== location.origin) continue;
      const params = new URLSearchParams(url.hash.slice(1));
      if (params.get("access") === token) continue;
      params.set("access", token); url.hash = params.toString(); link.href = url.href;
    }
  };
  new MutationObserver(decorate).observe(document.documentElement, { subtree: true, childList: true, attributes: true, attributeFilter: ["href"] });
  decorate();
})();
