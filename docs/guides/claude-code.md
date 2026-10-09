# How to share a Claude Code session

For `slink` 0.9.0 · updated 2026-10-09.

Run `slink view` in a project to read a Claude Code session in your browser,
then publish the part you choose as an end-to-end encrypted link. The session
is the transcript Claude Code already keeps: your messages, its answers, the
tool calls and their results. Nothing is uploaded until you publish, and the
person you send the link to needs only a browser.

## Read the session

```bash
slink view --from claude-code
```

`slink` finds the conversation history Claude Code keeps for this project,
under `~/.claude/projects` or the directory `CLAUDE_CONFIG_DIR` names. It takes
a snapshot of the session you choose and opens it in a local web reader. With
several sessions it shows a list to pick from. No account is needed.

The reader opens on your latest message and Claude's answer to it. The tool
calls, their results and any recorded reasoning sit collapsed under each answer
as **Agent activity**, and search covers one exchange or the whole session.
[Read a session](../user-guide.md#read-a-session) in the user guide describes
the reader in full. [Install](../user-guide.md#install) has the three ways to
get `slink`.

## Share it as a link

In the reader, choose **Share this view**. The panel starts with your message
and Claude's answer and shows exactly what will be shared. Add a title and a
comment if the reader will need them, then choose **Publish link** and who can
open it:

- **Anyone with the link.** The link carries its own key after the `#`, which
  never reaches the server.
- **Specific people.** Up to ten email addresses. Each person signs in with
  that address to open the share.

The first time you publish, you sign in with email or GitHub. What you prepared
is still there afterwards.

From the terminal, this publishes the newest Claude Code session for the
project, whole, after showing it and asking:

```bash
slink share --from claude-code
```

## Share only part of a conversation

A shared view holds only what you include. In the **Share this view** panel you
can remove pieces, add the agent's activity, or narrow a piece to a passage. To
share one passage, select the text in the reader and choose **Comment and
share**. The rest of the session stays on your machine.

## What a Claude Code session contains

- The session is labelled **Reconstructed**, because it is read from Claude
  Code's own transcript and not recorded on the wire.
- A sub-agent's work appears under the `Agent` call that started it.
- The summary Claude Code writes when it compacts a conversation, and its
  background-task notifications, are shown as provided context and not as
  something you typed.
- A response Claude Code wrote as several entries reads as one call, with its
  token usage counted once.

`slink` is checked against Claude Code 2.1.292. The
[handoff design](../handoff-design.md) lists the evidence and the limits.

## What Claude Code does by itself

Claude Code's `/export` command writes the current conversation as plain text,
to a file or the clipboard
([its command reference](https://code.claude.com/docs/en/commands), read
2026-10-09). That gives you a file to send, not a link. `slink` reads the same
conversation with each tool call and result kept as its own step, and publishes
it as a link. [Tools compared](compare.md) sets it beside the other ways to
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

### Does the person I send it to need Claude Code or slink?

No. They open the link in a browser.

### Is my transcript uploaded when I run slink view?

No. `slink view` reads the files on your disk and serves the reader from your
own machine. Uploading happens only when you choose **Publish link** or run
`slink share`.

### Which sessions does it find?

The ones started in the directory you run it in, or in a parent of it. To open
a particular one, pass its ID or transcript path with `--session`.

### Can I read a session that is on a remote machine?

Yes. Run `slink view --no-browser --port 4400` there and forward the port over
SSH, as [On a remote machine](../user-guide.md#on-a-remote-machine) shows.
