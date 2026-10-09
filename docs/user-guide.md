# session.link user guide

For `slink` 0.9.0 · updated 2026-10-09.

`slink` reads the sessions your coding agent already keeps, shows them in a
local web reader, and publishes the part you choose as an encrypted link.
Nothing leaves your machine until you publish. This guide covers installing
it, reading and sharing a session, recording, what stays private, and running
it from scripts.

## Install

```bash
brew install lftherios/tap/slink                   # macOS, Linux
curl -fsSL https://session.link/install.sh | sh    # macOS, Linux
npm i -g session.link                              # macOS, Linux, Windows
```

All three install the same native binary. The installer script verifies the
download against the release's checksums and takes `SLINK_VERSION` and
`SLINK_INSTALL_DIR` if you need a particular version or location. With npm you
can also run it without installing: `npx session.link view`. A Windows zip and
`.deb`, `.rpm` and `.apk` packages are on the
[releases page](https://github.com/lftherios/session-link/releases).

`slink version` prints what you have. To upgrade, use the same channel again.

## Read a session

Run this in a project where you have used a coding agent:

```bash
slink view
```

It finds that project's sessions, takes a snapshot of the one you choose and
opens it in your browser. With one session it opens directly; with several it
shows a list to pick from. No account is needed and nothing is uploaded.

The reader opens on your latest message and the agent's answer to it.

- **Agent activity** sits collapsed under each answer: the tool calls, their
  results and any reasoning the agent recorded.
- **Search** covers the exchange in front of you (**This view**) or everything
  (**Whole session**).
- The pager and the **N of M** button move between your messages. **Focused**
  shows one exchange and **Full session** shows them all.
- **Session details** lists the source, times, tokens and models, with the raw
  document and a **Trace explorer** that shows each recorded call.

The reader shows what was recorded and says when something was not. A session
is labelled **Exact capture** when `slink` recorded it on the wire,
**Reconstructed** when it was read from an agent's own history, and
**Partial capture** when part of it is known to be missing.

Useful options:

| Option | What it does |
| --- | --- |
| `--from <agent>` | Look only at one agent's sessions, for example `--from codex`. |
| `--session <id or path>` | Open exactly this session. A wrong or ambiguous ID is an error, never a guess. |
| `--pick` | Always show the list, even with one session. |
| `--no-browser` | Print the local address without opening a browser. |
| `--port <n>` | Listen on a port you choose. |
| `--background` | Return to your shell while the reader keeps running. |

The reader runs on your own machine at an address that ends in an access key.
Treat that address as private: anyone who can reach your machine with the
complete address can read the session. Stop the reader with Ctrl-C, or with
**Stop viewer** on its sessions page.

### On a remote machine

Start the reader on a fixed port, then forward that port from your own
computer and open the printed address there:

```bash
slink view --no-browser --port 4400          # on the remote machine
ssh -N -L 4400:127.0.0.1:4400 <remote-host>  # on your computer
```

## Share

In the reader, choose **Share this view**. The panel shows exactly what will
be shared.

1. Check what is included. It starts with your message and the agent's
   answer. You can add the agent's activity, remove pieces, or narrow one to a
   passage.
2. Add a title and a comment if the reader will need them. Your comment is
   shown apart from the recorded session.
3. **Preview view** shows what the recipient will see. **Save locally** keeps
   the view without publishing it.
4. **Publish link**, then choose who can open it.

To share one passage, select the text in the reader and choose
**Comment and share**.

The first time you publish, you are asked to sign in with email or GitHub.
The account is free, and what you prepared is still there afterwards.

### Anyone with the link

The link looks like `https://session.link/s/<id>#key=…`. The part after `#` is
the key. It never reaches the server, so the complete link is the only thing
that opens the share, and anyone who has it can read it. The recipient needs a
browser and nothing else.

### Specific people

Enter up to ten email addresses. You get one invitation link for each person
and send it to them yourself; session.link sends no email. Each person signs
in with that email address and accepts. A forwarded invitation does not work
for anyone else.

Keep your local reader running until access is granted. If you close it, the
pending invitations resume the next time you open it. Invitations that are not
accepted expire after seven days.

**Shared with people**, at the top of the reader, lists who has access and
lets you revoke an invitation. Revoking stops new downloads. It cannot take
back a copy someone already has.

### From the terminal

```bash
slink share            # your newest session for this project
slink share --pick     # choose from recent sessions
```

`slink share` publishes a whole session as a link for anyone who has it. It
shows what it is about to publish and asks before it does. Choosing part of a
session, and sharing with specific people, are done in the reader.

### Take a share offline

```bash
slink delete https://session.link/s/<id>
```

The link stops working at once. You can delete only what you published, and a
copy someone already downloaded stays with them.

### What is checked before anything is uploaded

- The session is scanned for common credential formats: provider API keys,
  GitHub, Slack and Stripe tokens, AWS access key IDs, Google API keys and
  private key blocks. A match stops the publish and names the kind it found.
  This is a safety net; it does not recognise every secret.
- A published session can be at most 25 MiB. A long recorded session can be
  larger than that. Share a view of it from the reader instead.

## Record

You do not need to record anything to read your agent's existing sessions.
Recording is for programs that keep no history of their own, and for an exact
record of what was sent.

```bash
slink record -- python agent.py
```

This runs your command with its Anthropic and OpenAI calls routed through a
local recorder, and saves them as one session when the command ends. Your
command's output and exit code pass through unchanged.

The recorder sees a program that takes its endpoint from `ANTHROPIC_BASE_URL`
or `OPENAI_BASE_URL`. It records Anthropic Messages calls and OpenAI Chat
Completions and Responses calls; anything else passes through unrecorded. If it
reports `captured 0 LLM calls`, the program did not use those variables. Codex
is one such program: read its own sessions with `slink view --from codex`.

### Record all the time

```bash
slink setup              # install the recorder as a login service
eval "$(slink on)"       # route this shell through it
```

The recorder starts a new session after fifteen minutes of quiet.
`eval "$(slink off)"` stops routing a shell, `slink status` shows what is
running and what has been captured, `slink doctor` checks the setup, and
`slink tap --uninstall` removes the service.

Each time the always-on recorder starts, it deletes sessions in `~/.slink/runs`
that are more than 30 days old. That includes one-off recordings and imports.
Set `SLINK_RETAIN_DAYS` to change the window. `slink prune` deletes old
sessions on request, and `slink list` shows what you have.

## Supported agents

`slink view` and `slink import` read these without any setup. A session
belongs to the directory it was started in, and `slink` looks there and in
each parent directory.

| Agent | `--from` | Good to know |
| --- | --- | --- |
| Claude Code | `claude-code` | Sub-agent work appears under the call that started it. |
| Codex | `codex` | Cannot be recorded with `slink record`; its saved sessions read normally. |
| opencode | `opencode` | |
| pi | `pi` | The [pi extension](../packages/pi-extension/README.md) adds `/slink view` and `/slink` inside pi. |
| omp | `omp` | |
| DeepSeek Harness | `dsh` | `slink view` run from the agent's own shell opens the session that ran it. |
| Aider | `aider` | Each run in `.aider.chat.history.md` is a session. Aider keeps no token counts and one time per run; its own notices show what it did. |
| Hermes | `hermes` | Experimental. |

`slink import --from <agent>` converts an agent's newest session into a local
one without opening the reader. What each agent's history does and does not
contain is listed in the [handoff design](handoff-design.md).

## Privacy and security

- **Local first.** Recordings, snapshots and drafts stay in `~/.slink` until
  you publish. Reading needs no account and sends nothing.
- **Encrypted before upload.** Each share is encrypted on your machine with a
  fresh AES-256-GCM key. The server stores ciphertext and never receives the
  key.
- **The page that decrypts is served by session.link.** The recipient's browser
  does the decrypting with code from the site, so a share is as trustworthy as
  that code.
- **No API keys in recordings.** The recorder saves request and response
  bodies, not headers.
- **External images are not fetched.** A session that points at an image on
  another site shows it only when you click.
- **Older links.** Links published by `slink` 0.5.0 and earlier (`/r/…`) are
  unlisted but not encrypted.

### Recovery and devices

This is optional. Set it up to keep a backup of your share links and their
keys, and to open shares addressed to you on another device.

Open **Recovery and devices** in the reader. Setup shows a recovery key once:
save it somewhere safe, then confirm that you have. To add another device,
sign in on it, choose **Request device approval**, enter the code it shows on a
device you have already approved, then choose **Check approval** on the new
one. Requests expire after ten minutes. The recovery key works when no approved
device is left.

Removing a device changes the keys, so it cannot open anything new. It cannot
undo what that device already received. If you lose every approved device and
the recovery key, the backup cannot be opened by signing in alone; links you
still hold keep working.

The protocol behind this is described in
[identity and encryption](identity-encryption.md) and
[named-recipient sharing](named-recipient-sharing.md).

## Scripts and unattended runs

`slink push` is `slink share` without the questions.

```bash
export SLINK_HOME=/var/run/job-42 SLINK_API_KEY=rk_…
slink record --name "nightly 42" -- python agent.py
url=$(slink push --yes)
```

- With no terminal attached, `push` and `share` need `--yes`.
- The link is the only thing printed on standard output. Everything else goes
  to standard error, and a failure exits non-zero.
- `SLINK_HOME` gives a job its own data directory, so the newest session is
  that job's.
- `SLINK_API_KEY` is the key that `slink login` saves in
  `~/.slink/config.json`. Revoke keys on your
  [account page](https://session.link/account).
- `slink list --json` and `slink status --json` give machine-readable output.
- A publish interrupted by the network can be run again: the retry sends the
  same encrypted bytes and prints the link.

Several agents can share one always-on recorder: a client that sends an
`x-slink-session` header gets its own session, and `x-slink-label` names it.

Limits to plan for: `push` publishes whole sessions only, the 25 MiB limit
applies, a detected credential stops the publish, and sharing with specific
people is done in the reader.

## Command reference

`slink <command> -h` lists a command's flags.

| Command | What it does |
| --- | --- |
| `view` | Open agent sessions in the local reader. Alias: `open`. |
| `share` | Publish a session after showing it and asking. |
| `push` | Publish a session from a script. |
| `delete` | Take one of your published sessions offline. |
| `login`, `logout` | Sign in or out of the account that publishing needs. |
| `record -- <cmd>` | Run a command with its model calls recorded. Alias: `dev`. |
| `import` | Convert an agent's newest session without opening the reader. |
| `setup` | Install the always-on recorder, with a prompt. |
| `tap` | Run the always-on recorder; `--install`, `--uninstall`, `--stop`. |
| `on`, `off` | Route this shell through the recorder, or stop. |
| `list` | List local sessions, newest first. Alias: `ls`. |
| `status` | Show the recorder, this shell's routing, the account and sessions. |
| `doctor` | Check the recording setup. |
| `prune` | Delete old local sessions. |
| `completion` | Print shell completion for bash, zsh or fish. |
| `version`, `help` | Print the version, or the command list. |

## Files and settings

Everything `slink` keeps is under `~/.slink`, readable only by your user
account.

| Path | Contents |
| --- | --- |
| `runs/` | Recorded and imported sessions. |
| `previews/` | Snapshots the reader opened. |
| `drafts/` | Share drafts, saved views and the titles you gave sessions. |
| `shares/` | A receipt for each link you published, including its key. |
| `named-shares/` | Invitations and access for shares with specific people. |
| `identity/` | This device's keys, once recovery is set up. |
| `config.json` | Your API key and server. |
| `tap.log`, `viewer-*.log` | Logs from the recorder and from background readers. |

| Variable | Effect |
| --- | --- |
| `SLINK_HOME` | Use this directory instead of `~/.slink`. |
| `SLINK_SERVER` | Publish to another compatible server. `--server` does the same for one command. |
| `SLINK_API_KEY` | Use this key instead of the saved one. |
| `SLINK_RETAIN_DAYS` | How long the always-on recorder keeps sessions. Default 30. |
| `SLINK_UPSTREAM_ANTHROPIC`, `SLINK_UPSTREAM_OPENAI` | Where the recorder forwards calls, for a gateway or a local model server. |

## Troubleshooting

| What you see | What to do |
| --- | --- |
| `captured 0 LLM calls` | The program does not read the base-URL variables. Read its saved sessions with `slink view` instead. |
| `not signed in` | Run `slink login`, or set `SLINK_API_KEY` in a script. |
| `publish blocked — the session appears to contain credentials` | Share a view that leaves the credential out, or remove it from the local file it names. |
| `session exceeds the 25 MiB limit` | Share a view from the reader instead of the whole session. |
| `not a TTY — pass --yes` | Add `--yes` when publishing from a script. |
| "This link is missing its encryption key" | The link was cut short. Ask the sender for the complete link, including everything after `#`. |
| "This share is unavailable" | The sender deleted it. |
| The reader does not open on a remote machine | Use `--no-browser --port` and forward the port, as above. |

`slink doctor` checks the recorder end to end, and `slink status` shows what is
running.

## The format

A session is a `session/v0` JSON document: a flat list of spans for model
calls, tool calls and agents, with the provider's own payloads kept beside the
normalised messages. The format, its schema and a validator are published as
[`@session-link/format`](../packages/format), and the reader as
[`@session-link/viewer`](../packages/viewer), both MIT-licensed. A
`session/v0` file stays readable with or without the hosted service.
