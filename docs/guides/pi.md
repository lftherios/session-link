# How to share a pi coding agent session

For `slink` 0.9.0 · updated 2026-10-09.

Type `/slink view` inside pi, or run `slink view --from pi` in the project, to
read a pi session in your browser, then publish it as an end-to-end encrypted
link. The session is the transcript pi already keeps: your messages, its
answers, the tool calls and their results. Nothing is uploaded until you
publish, and the person you send the link to needs only a browser.

## From inside pi

The [pi extension](../../packages/pi-extension/README.md) adds two commands to
pi. To read the session you are in:

```
/slink view
```

It opens the current session in a local browser preview and returns control to
pi. Nothing is uploaded. Choose **Share this view** in the browser when you are
ready.

To publish the session directly:

```
/slink
```

It publishes the whole session as an encrypted link and copies the link to your
clipboard.

The extension is not on npm yet. Install it from a checkout of the
[session-link repository](https://github.com/lftherios/session-link) with
`pi install ./packages/pi-extension`. It needs `slink` 0.6.0 or later;
[Install](../user-guide.md#install) has the three ways to get it.

## From the terminal

```bash
slink view --from pi
```

`slink` finds the session history pi keeps for this project, under
`~/.pi/agent/sessions` or the directory `PI_CODING_AGENT_DIR` names. A session
directory set through `PI_CODING_AGENT_SESSION_DIR` or `sessionDir` is followed
too. It takes a snapshot of the session you choose and opens it in a local web
reader. No account is needed.

## Share it as a link

In the reader, choose **Share this view**. The panel starts with your message
and the agent's answer and shows exactly what will be shared. Add a title and a
comment if the reader will need them, then choose **Publish link** and who can
open it: anyone with the link, or up to ten people by email address.

To share one passage and not the whole conversation, select the text in the
reader and choose **Comment and share**. [Share](../user-guide.md#share) in the
user guide covers each step.

## What a pi session contains

- A session captured live by the extension is labelled **Exact capture**: each
  turn is recorded from pi's own hooks, with the assembled system prompt and
  the request as it was sent.
- A session read from pi's transcript is labelled **Reconstructed**. That
  includes a resumed, forked or reloaded session, because the live capture
  starts where the current run did.
- The transcript is read along the branch that ends at its last entry, which
  is the one pi resumes. Messages on branches you went back from are counted in
  the session's details and not shown as turns.
- Compaction and branch summaries are shown as provided context, and a name you
  gave the session is its title.

`slink` is checked against the session format of pi 0.67.68 and 1.0.4. The
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

### Does the person I send it to need pi or slink?

No. They open the link in a browser.

### Do I need an account?

Only to publish. Reading a session, in pi or from the terminal, needs none. Run
`slink login` once before the first `/slink`.

### Does this work in omp?

`slink view --from omp` reads oh-my-pi's sessions, which keep pi's transcript
format. Whether the pi extension loads in omp is untested.
