# Tools for sharing Claude Code and Codex sessions, compared

For `slink` 0.9.0 · each tool checked against its own documentation on 2026-10-09.

Five tools turn a Claude Code or Codex session into something you can send:
`slink`, claude-code-transcripts, claude-replay, claudereview and Claudebin.
They differ in what they read, whether you get a file or a link, where the
transcript ends up, and who can open it. This page states what each tool's own
documentation says, and "not documented" where it says nothing.

## At a glance

What each tool reads, and what you end up sending:

| Tool | Reads | What you send |
| --- | --- | --- |
| `slink` | Claude Code, Codex, opencode, pi, omp, DeepSeek Harness, Aider, Hermes (experimental) | A link to a hosted reader |
| claude-code-transcripts | Claude Code | Static HTML pages, or a GitHub Gist preview link |
| claude-replay | Claude Code, Cursor, Codex CLI, Gemini CLI, OpenCode, Kimi Code, Hermes Agent | One self-contained HTML file |
| claudereview | Claude Code, Codex CLI, Gemini CLI | A link to a hosted viewer |
| Claudebin | Claude Code | A link to a hosted page |

Where the session goes, and who can read it:

| Tool | Where it goes | Part of a session | Who can open it |
| --- | --- | --- | --- |
| `slink` | Encrypted on your machine, then uploaded; the server stores ciphertext | Yes: chosen exchanges or one passage, with your note | Anyone with the complete link, or up to ten people by email address |
| claude-code-transcripts | Your disk, or a Gist in your own GitHub account | Not documented | Whoever you give the pages or the Gist link to |
| claude-replay | Your disk; you send or host the file | Yes: a range of turns, excluded turns, redacted text | Whoever you give the file to |
| claudereview | Encrypted before upload, then stored on claudereview.com | Not documented | Anyone with the link, or anyone with the password you set |
| Claudebin | Published to claudebin.com | Not documented | Not documented |

## Which one fits

**You want a file and no service.** claude-replay and claude-code-transcripts
write HTML you can email or host yourself. `slink` does not write a standalone
HTML page. It keeps a session as a `session/v0` JSON document and serves the
reader itself, on your machine or from session.link.

**You want a link the server cannot read.** `slink` and claudereview both
encrypt the session before upload with AES-256-GCM and carry the key in the
part of the link after `#`, which a browser does not send to the server.

**You want to send part of a session.** In `slink` you choose the exchanges or
select a passage in the reader, add a note, and preview what the recipient will
see. claude-replay takes a range of turns and text to redact on the command
line.

**You want only certain people to open it.** `slink` can address a share to up
to ten email addresses, and each person signs in with that address to open it.
claudereview can protect a share with a password.

**You use more than one agent.** `slink` reads eight, claude-replay seven and
claudereview three. claude-code-transcripts and Claudebin are for Claude Code.

## What the agents do by themselves

- **Claude Code** has `/export`, which writes the current conversation as plain
  text to a file or the clipboard. It makes no link.
- **opencode** has `/share`, which creates a public URL for the session and
  syncs the conversation to opencode's servers. Anyone with the link can read
  it. `/unshare` removes the link and deletes that data.

## How to use slink

Run this in a project where you have used a coding agent:

```bash
slink view
```

It opens that project's sessions in a local web reader, with nothing uploaded.
Choose **Share this view** there to publish a link. The guides for
[Claude Code](claude-code.md), [Codex](codex.md), [opencode](opencode.md),
[Aider](aider.md) and [pi](pi.md) go through it for each agent, and the
[user guide](../user-guide.md) covers the rest.

## Sources

Each row is from the tool's own README or documentation, read on 2026-10-09.
Tools change; if a row is out of date, the source is the authority.

- [claude-code-transcripts](https://github.com/simonw/claude-code-transcripts), Apache-2.0
- [claude-replay](https://github.com/es617/claude-replay), MIT
- [claudereview](https://github.com/vignesh07/claudereview), MIT
- [Claudebin](https://github.com/wunderlabs-dev/claudebin), MIT
- [Claude Code commands](https://code.claude.com/docs/en/commands)
- [opencode: Share](https://opencode.ai/docs/share/)
- `slink`: the [user guide](../user-guide.md) and the
  [session-link repository](https://github.com/lftherios/session-link), MIT
