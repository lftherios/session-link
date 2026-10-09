# @session-link/pi-extension

Publish the [pi](https://github.com/earendil-works/pi) session you're in to a
permanent [session.link](https://session.link) URL — without leaving the TUI.

You're deep in a pi session, something interesting happened, and you want to
share it. Type `/slink`. The extension captures the current session, publishes
it as an end-to-end encrypted link, and hands you back the URL.

## Install

Install the `slink` CLI first; it does the capture and the publishing. The
[main README](../../README.md#install) lists the channels. The extension needs
slink 0.6.0 or later (`slink --version`): `/slink view` and encrypted links both
arrived in that release.

The extension is not on npm yet. Install it from a checkout of this
repository:

```bash
pi install ./packages/pi-extension
```

`pi install` adds the extension to your pi settings and loads it. If `slink`
is not on your `PATH`, the extension falls back to `npx --yes session.link`,
which runs whatever version npm serves; that version must also be 0.6.0 or
later.

## Use

In a pi session:

```
/slink view
```

Opens the current session in a local browser preview and returns control to pi.
It uses the persisted transcript to include earlier turns in resumed sessions
(`reconstructed` fidelity), falling back to the live capture for in-memory
sessions. Nothing is uploaded. Review the saved snapshot and choose **Share
this view** in the browser when ready. Stop the background viewer from its
sessions page.

To publish directly:

```
/slink
```

→ `session.link: published → https://session.link/s/9f3kx2mvq7wtd4#key=…` (also
copied to your clipboard).

`/slink` publishes the whole session. A session started in this pi run is
published from the live capture. One you resumed had turns before this run,
so it is published from pi's own transcript instead.

## Notes

- **Private by design.** Nothing leaves your machine until you run `/slink`.
  Publishing goes through the CLI's own gate — the session is validated and
  **secret-scanned** (`sk-…`, `ghp_…`, `AKIA…`, PEM blocks); a hit blocks the
  publish. The session is encrypted on your machine, and anyone with the
  complete link can view it.
- **Sign-in.** Publishing to session.link needs an account. Run `slink login`
  once; until then `/slink` reports that you are not signed in. Capturing and
  `/slink view` never need one.
- **Auto-publish (opt-in).** Set `SLINK_AUTOPUBLISH=1` and each session
  publishes itself when it ends — the trace link appears without your typing
  `/slink`. Off by default; capture is always local until then.
- **Self-hosting / config.** `SLINK_SERVER` targets a different server;
  `SLINK_BIN` overrides how the CLI is invoked (e.g. `SLINK_BIN="npx --yes session.link"`).
- **Fidelity.** Sessions captured live run at **`exact`** fidelity: the
  extension records each turn from pi's in-process SDK hooks, including the
  assembled system prompt and the verbatim provider request (kept in
  `raw.request`). Resumed, forked and reloaded sessions are published from
  pi's transcript at **`reconstructed`** fidelity, because the live capture
  starts where this run did.

MIT © [session.link](https://session.link)
