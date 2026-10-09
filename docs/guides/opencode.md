# How to share an opencode session

For `slink` 0.9.0 · updated 2026-10-09.

Run `slink view --from opencode` in a project to read an opencode session in
your browser, then publish the part you choose as an end-to-end encrypted link.
The session is what opencode already stores: your messages, its answers, the
tool calls and their results. Nothing is uploaded until you publish, and the
person you send the link to needs only a browser.

## Read the session

```bash
slink view --from opencode
```

`slink` reads opencode's session database, `opencode.db` in
`~/.local/share/opencode` or under `XDG_DATA_HOME`, and picks out the sessions
started in this project. `OPENCODE_DB` names another database. It takes a
snapshot of the session you choose and opens it in a local web reader. No
account is needed.

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

From the terminal, this publishes the newest opencode session for the project,
whole, after showing it and asking:

```bash
slink share --from opencode
```

To share one passage and not the whole conversation, select the text in the
reader and choose **Comment and share**. [Share](../user-guide.md#share) in the
user guide covers each step.

## What an opencode session contains

- The session is labelled **Reconstructed**, because it is read from
  opencode's database and not recorded on the wire.
- Text, reasoning, tool calls, their results and token usage are shown as
  opencode recorded them.
- A sub-agent's work is a child session, shown under the `task` call that
  records its ID.
- Text opencode wrote into one of your messages itself, such as its account of
  reading an attached file, is shown as provided context.
- opencode's patch events are not a saved final diff, so the reader does not
  present one.

`slink` is checked against opencode 1.18.35, whose sessions are in the
`message` and `part` tables. A second store that opencode keeps for a newer
session engine is not read. The [handoff design](../handoff-design.md) lists
the evidence and the limits.

## How this differs from opencode's own share command

opencode has a `/share` command. It creates a public URL for the session and
syncs the conversation to opencode's servers, where anyone with the link can
read it, and `/unshare` removes the link and deletes that data
([opencode's documentation](https://opencode.ai/docs/share/), read
2026-10-09). With `slink` the session is encrypted on your machine before
upload, you can share part of it, and you can limit a share to people you name
by email address. [Tools compared](compare.md) sets it beside the other ways to
share a session.

## Privacy

- Reading sends nothing. The reader runs on your own machine.
- A share is encrypted on your machine with its own AES-256-GCM key before
  upload. The server stores ciphertext.
- Before upload the session is scanned for common credential formats, and a
  match stops the publish. This is a safety net, not a guarantee.
- `slink delete` with the link takes a share offline.

[Privacy and security](../user-guide.md#privacy-and-security) has the detail.

## Questions

### Does the person I send it to need opencode or slink?

No. They open the link in a browser.

### Is my session uploaded when I run slink view?

No. `slink view` reads the database on your disk and serves the reader from
your own machine. Uploading happens only when you choose **Publish link** or
run `slink share`.

### Can I open one particular opencode session?

Yes. Pass its session ID with `--session`. A wrong or ambiguous ID is an error,
never a guess.
