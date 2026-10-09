# How to share a Codex CLI session

For `slink` 0.9.0 · updated 2026-10-09.

Run `slink view --from codex` in a project to read a Codex session in your
browser, then publish the part you choose as an end-to-end encrypted link. The
session is the rollout Codex already saves: your messages, its answers, the
shell commands, patches and their results. Nothing is uploaded until you
publish, and the person you send the link to needs only a browser.

## Read the session

```bash
slink view --from codex
```

`slink` finds the session history Codex keeps under `~/.codex/sessions`, or
under `CODEX_HOME` when that is set, and picks out the sessions started in this
project. It takes a snapshot of the one you choose and opens it in a local web
reader. No account is needed.

The reader opens on your latest message and the answer to it. The tool calls,
their results and any recorded reasoning sit collapsed under each answer as
**Agent activity**, and search covers one exchange or the whole session.
[Read a session](../user-guide.md#read-a-session) in the user guide describes
the reader in full. [Install](../user-guide.md#install) has the three ways to
get `slink`.

## Share it as a link

In the reader, choose **Share this view**. The panel starts with your message
and the agent's answer and shows exactly what will be shared. Add a title and a
comment if the reader will need them, then choose **Publish link** and who can
open it: anyone with the link, or up to ten people by email address.

From the terminal, this publishes the newest Codex session for the project,
whole, after showing it and asking:

```bash
slink share --from codex
```

To share one passage and not the whole transcript, select the text in the
reader and choose **Comment and share**. [Share](../user-guide.md#share) in the
user guide covers each step.

## What a Codex session contains

- The session is labelled **Reconstructed**, because it is read from the
  rollout file and not recorded on the wire.
- Function calls, `apply_patch`, local shell commands, tool search and web
  search appear as tool calls in the order Codex recorded them. Any other
  rollout item is kept as it was written.
- A sub-agent thread appears under the `spawn_agent` call that started it.
- Rollouts compressed to `.jsonl.zst` are not found. Codex 0.160.1 writes them
  only behind a flag that is off by default.

`slink` is checked against Codex 0.160.1. The
[handoff design](../handoff-design.md) lists the evidence and the limits.

## Why slink record captures nothing from Codex

`slink record` and the always-on recorder route a program through a local
recorder by setting `OPENAI_BASE_URL`. Codex does not read that variable, so
the recorder reports `captured 0 LLM calls`. You do not need to record Codex:
its saved sessions read normally with `slink view --from codex`.

## Privacy

- Reading sends nothing. The reader runs on your own machine.
- A share is encrypted on your machine with its own AES-256-GCM key before
  upload. The server stores ciphertext.
- Before upload the session is scanned for common credential formats, and a
  match stops the publish. This is a safety net, not a guarantee.
- `slink delete` with the link takes a share offline.

[Privacy and security](../user-guide.md#privacy-and-security) has the detail.

## Questions

### Does the person I send it to need Codex or slink?

No. They open the link in a browser.

### Is my session uploaded when I run slink view?

No. `slink view` reads the files on your disk and serves the reader from your
own machine. Uploading happens only when you choose **Publish link** or run
`slink share`.

### Can I open one particular Codex session?

Yes. Pass its ID or the path of its rollout file with `--session`. A wrong or
ambiguous ID is an error, never a guess.
