// Demo backend for assets/demo.tape — lets the GIF record a REAL slink capture
// with no API keys and no network: it mocks the OpenAI upstream the proxy
// forwards to, and the ingest endpoint `slink push` posts to. Not shipped to
// users; only used to render the demo.
import http from "node:http";

// One streamed OpenAI chat.completion, token by token, with a usage tail.
function stream(res, { id, text, inTok, outTok }) {
  res.writeHead(200, { "content-type": "text/event-stream", "cache-control": "no-cache" });
  const base = { id, object: "chat.completion.chunk", created: 1, model: "gpt-4.1" };
  const send = (o) => res.write(`data: ${JSON.stringify(o)}\n\n`);
  send({ ...base, choices: [{ index: 0, delta: { role: "assistant" }, finish_reason: null }] });
  for (const tok of text.match(/\S+\s*/g) ?? [text])
    send({ ...base, choices: [{ index: 0, delta: { content: tok }, finish_reason: null }] });
  send({ ...base, choices: [{ index: 0, delta: {}, finish_reason: "stop" }] });
  send({ ...base, choices: [], usage: { prompt_tokens: inTok, completion_tokens: outTok, total_tokens: inTok + outTok } });
  res.write("data: [DONE]\n\n");
  res.end();
}

const turns = [
  { text: "I'll compute 12% of $86.40 with the calculator, then round to cents.", inTok: 812, outTok: 190 },
  { text: "The tip is $10.37, for a total of $96.77.", inTok: 1006, outTok: 44 },
];
let n = 0;

http
  .createServer(async (req, res) => {
    for await (const _ of req) {} // drain
    if (req.url === "/v1/chat/completions") return stream(res, { id: `cc-${++n}`, ...turns[Math.min(n - 1, turns.length - 1)] });
    if (req.url === "/api/runs") {
      res.writeHead(201, { "content-type": "application/json" });
      return res.end(JSON.stringify({ id: "9f3kx2mvq7wt", url: "https://session.link/r/9f3kx2mvq7wt", hash: "sha256:a9ffc7760b1d2f4b", deduplicated: false }));
    }
    res.writeHead(200);
    res.end("ok");
  })
  .listen(9099, "127.0.0.1");
