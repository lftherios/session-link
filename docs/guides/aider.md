# How to share an Aider chat history

For `slink` 0.9.0 · updated 2026-10-09.

Run `slink view --from aider` in a project to read an Aider session in your
browser, then publish the part you choose as an end-to-end encrypted link. A
session is one run from `.aider.chat.history.md`: what you typed, the model's
replies, and Aider's own notices of the edits, commits and test output. Nothing
is uploaded until you publish, and the person you send the link to needs only a
browser.

## Read the session

```bash
slink view --from aider
```

Aider has no session store. It appends every run in a project to
`.aider.chat.history.md`, in the directory it was started in or the git root
when there is one. `slink` treats each run in that file as a session, named by
the start time Aider wrote for it. It takes a snapshot of the one you choose
and opens it in a local web reader. No account is needed.

The reader opens on your latest message and the reply to it, and search covers
one exchange or the whole session.
[Read a session](../user-guide.md#read-a-session) in the user guide describes
the reader in full. [Install](../user-guide.md#install) has the three ways to
get `slink`.

## Share it as a link

In the reader, choose **Share this view**. The panel starts with your message
and the reply and shows exactly what will be shared. Add a title and a comment
if the reader will need them, then choose **Publish link** and who can open it:
anyone with the link, or up to ten people by email address.

From the terminal, this publishes the newest Aider run for the project, whole,
after showing it and asking:

```bash
slink share --from aider
```

To share one passage and not the whole chat, select the text in the reader and
choose **Comment and share**. [Share](../user-guide.md#share) in the user guide
covers each step.

## What an Aider session contains

Aider's history holds less than other agents record, and nothing is made up for
it.

- The session is labelled **Reconstructed**, because it is read from the
  history file and not recorded on the wire.
- There are no tool calls by the model. Aider's edits are text in the reply,
  so its notices stand as the evidence of what it did: the edits it applied,
  its commits, and its lint and test output.
- There are no token counts. Aider prints them rounded, and they stay in its
  notices as written.
- There is one time, the start of the run. Where `.aider.input.history` is
  beside the file, a typed entry takes its time from there.
- The output of `/run` and any image are not in the history, so they are not in
  the session.
- The model and Aider's version are read from the notices at the start of the
  run.

`slink` is checked against Aider 0.86.2. The
[handoff design](../handoff-design.md) lists the evidence and the limits.

## Privacy

- Reading sends nothing. The reader runs on your own machine.
- A share is encrypted on your machine with its own AES-256-GCM key before
  upload. The server stores ciphertext.
- Before upload the session is scanned for common credential formats, and a
  match stops the publish. This is a safety net, not a guarantee.
- `slink delete` with the link takes a share offline.

[Privacy and security](../user-guide.md#privacy-and-security) has the detail.

## Questions

### Does the person I send it to need Aider or slink?

No. They open the link in a browser.

### Is my chat history uploaded when I run slink view?

No. `slink view` reads the file on your disk and serves the reader from your
own machine. Uploading happens only when you choose **Publish link** or run
`slink share`.

### My history file has another name. Is it found?

Not by itself. A history that Aider was told to keep under another name, by an
option, an environment variable or a configuration file, is not looked for.
Give its path with `--session` and it is read.
