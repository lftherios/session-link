<div align="center">

# session.link

**Turn any LLM session — an eval item, an agent trace, a one-off completion — into a permanent, shareable URL you can inspect.**

[![npm](https://img.shields.io/npm/v/session.link?color=0e6f5c&label=session.link)](https://www.npmjs.com/package/session.link)
[![downloads](https://img.shields.io/npm/dm/session.link?color=0e6f5c)](https://www.npmjs.com/package/session.link)
[![CI](https://img.shields.io/github/actions/workflow/status/lftherios/session-link/ci.yml?branch=main&label=ci)](https://github.com/lftherios/session-link/actions)
[![format](https://img.shields.io/badge/format-session%2Fv0-0e6f5c)](https://github.com/lftherios/session-link/tree/main/packages/format)
[![license](https://img.shields.io/npm/l/session.link?color=0e6f5c)](https://github.com/lftherios/session-link/blob/main/LICENSE)

![slink capturing an agent run and publishing it to a shareable URL](https://raw.githubusercontent.com/lftherios/session-link/main/assets/demo.gif)

**[▶ Open a sample session in the viewer →](https://session.link/r/agent-eval)**

</div>

## Why

Agent runs are ephemeral. When something interesting happens — a clever tool call, a wrong turn, a great eval result — your options are a screenshot that loses the tree, timing, and cost, or a wall of pasted JSON nobody will read. `slink` gives you a third option: **a link.** Capture is ambient and local; publishing is deliberate; the result is a permanent page a teammate can actually open.

## Install

`slink` is a single native binary — no runtime to install. Three ways to get it:

```bash
brew install lftherios/tap/slink                   # macOS / Linux
curl -fsSL https://session.link/install.sh | sh    # any platform, checksum-verified
npm i -g session.link                              # or `npx session.link`
```

`brew` and `curl` need no Node at all. The `npm`/`npx` path installs the *same* native binary (npm fetches only the ~5 MB package for your platform, then a tiny Node launcher execs it). Windows `.zip`, Linux `.deb`/`.rpm`/`.apk`, and `checksums.txt` are on the [releases page](https://github.com/lftherios/session-link/releases).

## Quickstart

```bash
# 1. Record — wrap your agent. Nothing leaves your machine.
slink dev -- python agent.py

# 2. Publish — one-time GitHub sign-in, then validate, secret-scan, confirm.
slink login && slink push
# → https://session.link/r/9f3kx2mvq7wtd4   (copied to your clipboard)
```

That's it — no code changes, no SDK. Capturing needs no account at all; `login` (free, GitHub) is only for publishing, so sessions are attributed and deletable by you. `slink dev` points `ANTHROPIC_BASE_URL` / `OPENAI_BASE_URL` at a local recording proxy, runs your command, and writes each call to `~/.slink` as it happens — streaming passed through untouched and reassembled.

- **Node agent?** `slink dev -- node agent.js` — anything that speaks the Anthropic or OpenAI API.
- **Local models?** The proxy can point at any compatible upstream: `SLINK_UPSTREAM_OPENAI=http://localhost:11434 slink dev -- …` records Ollama sessions too. And if your shell already has `OPENAI_BASE_URL` pointed somewhere custom, `slink on` keeps forwarding there — your traffic is never silently rerouted.
- **Already ran it in Claude Code, Codex, opencode, pi, or Hermes?** `slink import` — [see below](#works-with-the-agent-you-already-use).
- **Review before sharing?** `slink open` browses your captures locally in the exact viewer the hosted site renders, Publish button included.

## Always on (optional)

Wrapping each run with `slink dev` is the simplest way in, and all you need. If you'd rather not think about it, you can instead leave a recorder running in the background — a convenience, not a requirement:

```bash
slink tap --install    # persistent recorder as a login service — survives reboots
eval "$(slink on)"     # route this shell's agents through it
```

Now everything flowing through is recorded in the background, grouped into **activity windows**: traffic separated by an idle gap (15 minutes by default) becomes its own local capture, and a client can tag its calls into a named window of its own with the `x-slink-session` / `x-slink-label` headers (the idle gap still closes a window that goes quiet). Two untagged agents talking at the same moment share a window — true per-process segmentation is on the roadmap. Publish any one later with `slink push --pick`; `slink share` imports your coding agent's newest session and publishes it in one step. Publish any one later with `slink share` (import-or-pick the newest capture and `push` it, in one step). Captures are plaintext on your disk and auto-pruned after 30 days (`SLINK_RETAIN_DAYS`); `slink prune` trims the buffer on demand. Stop routing a shell with `eval "$(slink off)"`, or remove the service entirely with `slink tap --uninstall`.

## Works with the agent you already use

Didn't wrap it in `slink dev`? Import it after the fact. `slink import` reconstructs a `session/v0` capture straight from your coding agent's own on-disk history — no proxy, no re-run. With no arguments it grabs the newest session for the current project, whichever agent produced it; `--from` pins one:

| Agent | Where its sessions live | Import |
| --- | --- | --- |
| <img src="https://raw.githubusercontent.com/lftherios/session-link/main/assets/logos/claude-code.png" height="16" align="center"> &nbsp;[Claude Code](https://claude.com/claude-code) | `~/.claude/projects/…` (JSONL) | `slink import --from claude-code` |
| <img src="https://raw.githubusercontent.com/lftherios/session-link/main/assets/logos/codex.png" height="16" align="center"> &nbsp;[Codex](https://github.com/openai/codex) | `~/.codex/sessions/**/rollout-*.jsonl` | `slink import --from codex` |
| <img src="https://raw.githubusercontent.com/lftherios/session-link/main/assets/logos/opencode.svg" height="16" align="center"> &nbsp;[opencode](https://opencode.ai) | `~/.local/share/opencode/opencode.db` (SQLite) | `slink import --from opencode` |
| <img src="https://raw.githubusercontent.com/lftherios/session-link/main/assets/logos/pi.svg" height="16" align="center"> &nbsp;[pi](https://github.com/badlogic/pi-mono) | `~/.pi/agent/sessions/…` (JSONL) | `slink import --from pi` |
| <img src="https://raw.githubusercontent.com/lftherios/session-link/main/assets/logos/hermes.png" height="16" align="center"> &nbsp;[Hermes](https://github.com/NousResearch/hermes-agent) <sup>experimental</sup> | `~/.hermes/state.db` (SQLite) | `slink import --from hermes` |

```bash
slink import   # newest session in this repo, whichever agent produced it
slink push     # → a link   (or `slink share`: both steps in one)
```

Imports are marked **`fidelity: reconstructed`** — the transcript carries the messages, tool calls, models, and token usage, but not the raw wire request bodies (a capture from `slink dev` is `exact`). The SQLite-backed stores (opencode, Hermes) are read directly by the binary — no extra dependencies. Hermes is **experimental** — its `tool_calls` shape is inferred, not yet verified against a live session.

**Using pi? Skip import entirely.** The [`@session-link/pi-extension`](https://github.com/lftherios/session-link/tree/main/packages/pi-extension) adds a `/slink` command to pi — publish the session you're in without leaving the TUI, at **`exact`** fidelity: it records each turn from pi's in-process SDK hooks, assembled system prompt and verbatim provider request included.

## What a shared link gives you

A published session isn't a screenshot — it's the real thing, rendered:

- 🌳 **An interactive trace tree** with a timing micro-waterfall — spans colored by kind (llm_call, tool_call, retrieval, agent), collapsible, navigable. Windowed rendering keeps it smooth on real sessions: a 12MB, 4,000-span working day scrolls like a toy example.
- 💬 **Formatted messages** — system / user / assistant / thinking / tool calls, with a **Raw JSON** toggle one click away (normalization is never lossy; the raw payload is preserved).
- 📊 **Token, cost, latency, and score chips** rolled up per run and per span.
- 🔗 **`#span=` deep links** — point a teammate at step 14, not "scroll down a bit."
- 💌 **Slack / OG unfurls** — the link describes itself in chat, no click required.

## Private by design

`slink` runs in the path of your prompts and API keys, so it's built to be safe to run on real work — and honest about where the edges are:

- **Capture is 100% local.** A recording proxy tees the calls to disk; nothing is uploaded until you run `push`.
- **API keys never touch the capture.** The proxy records request/response bodies only — auth headers are forwarded upstream and dropped, so they can't end up in a published session.
- **Secrets are scanned twice** — client-side before a single byte leaves your machine, and again server-side before anything touches disk. The scan is deliberately high-precision, low-recall: [a short list of unambiguous key formats](https://github.com/lftherios/session-link/blob/main/packages/format/secret-patterns.mjs) (`sk-…`, `ghp_…`, `AKIA…`, Stripe, PEM blocks), not DLP. It catches a pasted key; it can't know which *content* is sensitive — that's what reviewing in `slink open` before publishing is for.
- **Sessions are immutable and content-addressed** — the exact bytes are served back, so anyone can verify: `curl -s https://session.link/api/runs/<id>/raw | shasum -a 256`.
- **Deletion is a tombstone, and publishes are owned.** Deleting (only your account can) takes the page, unfurls, and raw bytes offline immediately. The underlying blob is currently retained server-side — re-publishing identical bytes from your account revives the same URL — so treat publishing as hard to fully un-ring; a true purge path is on the roadmap.
- **Unlisted is not access control.** ~69 bits of unguessable URL, no public index, crawlers excluded — the share-a-doc-link model. Anyone holding the link can read the session; keep genuinely sensitive runs local.

### If session.link disappears

Your captures never leave `~/.slink` unless you publish them. The [format](https://github.com/lftherios/session-link/tree/main/packages/format) and [viewer](https://github.com/lftherios/session-link/tree/main/packages/viewer) are MIT — a `session/v0` file renders forever, with or without the hosted service — and every published session's exact bytes are one `curl` away for backup. `slink push --server <url>` (or `SLINK_SERVER`) points the client at any compatible sink; session.link is the default, not a hardcode.

## What's in the box

An open client and an open format, not a thin wrapper around a hosted API:

| Component | What it is |
| --- | --- |
| [`slink`](https://github.com/lftherios/session-link/tree/main/go) | the CLI + always-on tap — recording proxy, importer, local viewer, publish flow. A single native Go binary (~7MB, no runtime). |
| [`@session-link/format`](https://github.com/lftherios/session-link/tree/main/packages/format) | the open `session/v0` format — TypeScript types, JSON Schema, and `validateRun`. |
| [`@session-link/viewer`](https://github.com/lftherios/session-link/tree/main/packages/viewer) | the React trace-tree component that renders a session — the same one the hosted site uses, embedded in the CLI for `slink open`. |

The CLI is Go as of v0.3.0 (previously a Node bundle; the pre-Go source is tagged `js-cli-v0.2`). `@session-link/format` and `@session-link/viewer` stay on npm — the hosted server and any embedder consume them.

The `session/v0` wire format is open-world: unknown span types, content parts, roles, and extra fields all validate and round-trip, so it degrades gracefully as providers and frameworks evolve. It's pre-1.0 and will change before it's frozen.

## Roadmap

Today, one link lets you **inspect**. Next, anchored to the same URLs:

- **Remix** — fork any call into a playground, change the prompt or model, re-run on your own key. *(soon)*
- **Compare** — diff two runs sharing a `group` id; scores line up on their own. *(soon)*
- **Discuss** — comments that anchor to spans, not screenshots. *(soon)*

## Contributing

Issues and PRs welcome. The CLI is Go — `cd go && go test ./...`. For the format and viewer packages, `npm install` then `npm test`; `npm run build:viewer` rebuilds the bundle the Go CLI embeds. The demo GIF regenerates from a checked-in [VHS](https://github.com/charmbracelet/vhs) tape: `vhs assets/demo.tape`.

The hosted service lives at **[session.link](https://session.link)**; this repo is the open client and format.

## License

MIT © [session.link](https://session.link) — see [LICENSE](https://github.com/lftherios/session-link/blob/main/LICENSE).

---

Also on [Radicle](https://radicle.xyz), the peer-to-peer code network:

```
rad:z24FnLsshNV8kq5fWCTi2dbKueomr
```

Clone it with `rad clone rad:z24FnLsshNV8kq5fWCTi2dbKueomr`.
