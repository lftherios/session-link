// A tiny "math agent" for the demo — makes two model calls through whatever
// OPENAI_BASE_URL points at. Nothing here is slink-specific: slink just wraps
// the process and points that env var at its local recording proxy.
const base = process.env.OPENAI_BASE_URL ?? "https://api.openai.com/v1";

async function ask(messages) {
  const res = await fetch(`${base}/chat/completions`, {
    method: "POST",
    headers: { "content-type": "application/json", authorization: "Bearer sk-demo" },
    body: JSON.stringify({ model: "gpt-4.1", stream: true, stream_options: { include_usage: true }, messages }),
  });
  let text = "";
  const dec = new TextDecoder();
  for await (const chunk of res.body) {
    for (const line of dec.decode(chunk).split("\n")) {
      if (!line.startsWith("data:")) continue;
      const p = line.slice(5).trim();
      if (!p || p === "[DONE]") continue;
      try { text += JSON.parse(p).choices?.[0]?.delta?.content ?? ""; } catch {}
    }
  }
  return text;
}

console.log("math-agent · q117: what's a 12% tip on $86.40?");
console.log("  ↳ " + (await ask([{ role: "user", content: "What's a 12% tip on $86.40? Plan first." }])));
console.log("  ↳ calculator(86.40 * 0.12) = 10.368");
console.log("  ✓ " + (await ask([{ role: "user", content: "Given tip 10.368, give the final answer." }])));
