# @session-link/pi-extension

Publish the [pi](https://github.com/badlogic/pi-mono) session you're in to a
permanent [session.link](https://session.link) URL — without leaving the TUI.

You're deep in a pi session, something interesting happened, and you want to
share it. Type `/slink`. The extension captures the current session, publishes
it unlisted, and hands you back a link.

## Install

```bash
npm i -g session.link        # the slink CLI does the capture + publish
pi install @session-link/pi-extension
```

`pi install` adds the extension to your pi settings and loads it. If you'd
rather not install `slink` globally, the extension falls back to
`npx --yes session.link` automatically.

## Use

In a pi session:

```
/slink
```

→ `session.link: published → https://session.link/r/9f3kx2mvq7wt` (also copied
to your clipboard).

## Notes

- **Private by design.** Nothing leaves your machine until you run `/slink`.
  Publishing goes through the CLI's own gate — the session is validated and
  **secret-scanned** (`sk-…`, `ghp_…`, `AKIA…`, PEM blocks); a hit blocks the
  publish. Links are unlisted by default.
- **Attribution.** Run `slink login` (GitHub) once so published sessions are
  attributed to you; otherwise they're anonymous.
- **Auto-publish (opt-in).** Set `SLINK_AUTOPUBLISH=1` and each session
  publishes itself when it ends — the trace link appears without your typing
  `/slink`. Off by default; capture is always local until then.
- **Self-hosting / config.** `SLINK_SERVER` targets a different server;
  `SLINK_BIN` overrides how the CLI is invoked (e.g. `SLINK_BIN="npx --yes session.link"`).
- **Fidelity.** Sessions captured live run at **`exact`** fidelity: the
  extension records each turn from pi's in-process SDK hooks, including the
  assembled system prompt and the verbatim provider request (kept in
  `raw.request`). Resumed sessions with no live turns fall back to
  `slink import` (`reconstructed`) so `/slink` always works.

MIT © [session.link](https://session.link)
