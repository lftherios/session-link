<div align="center">

# session.link

**Turn coding-agent sessions into links your colleagues can actually read.**

[![CI](https://img.shields.io/github/actions/workflow/status/lftherios/session-link/ci.yml?branch=main&label=ci)](https://github.com/lftherios/session-link/actions)
[![npm](https://img.shields.io/npm/v/session.link?color=0e6f5c&label=session.link)](https://www.npmjs.com/package/session.link)
[![license](https://img.shields.io/badge/license-MIT-0e6f5c)](LICENSE)

[![The session.link reader: a human input, the agent's response and its collapsed activity](https://raw.githubusercontent.com/lftherios/session-link/main/assets/product/focused.webp)](https://session.link/demo)

**[Try it on a sample session →](https://session.link/demo)**

</div>

`slink` opens the sessions of the agent you already use in a local web reader.
Pick the exchange that matters, add a note, and publish an end-to-end encrypted
link. Nothing leaves your machine until you do.

## Install

```bash
brew install lftherios/tap/slink                   # macOS, Linux
curl -fsSL https://session.link/install.sh | sh    # any platform, checksum-verified
npm i -g session.link                              # the same native binary via npm
```

Windows, `.deb`, `.rpm` and `.apk` packages are on the
[releases page](https://github.com/lftherios/session-link/releases).

## Use

```bash
slink view
```

`slink view` finds this project's sessions from Claude Code, Codex, opencode, pi,
omp, DeepSeek Harness, Aider and Hermes (experimental), with no SDK and no
re-run. To share one, choose **Share this view**: keep what matters, add a note,
then publish a link for anyone who has it or for specific people by email.

```bash
slink record -- python agent.py    # record any Anthropic or OpenAI API client, wire-exact
slink import --from codex          # convert an agent's newest session without the viewer
slink share                        # publish from the terminal instead
slink tap --install                # optional always-on recorder; then eval "$(slink on)"
```

In pi, the [extension](packages/pi-extension) adds `/slink view` and `/slink`.
Run `slink help` for everything else. On a remote machine, run
`slink view --no-browser --port 4400` and forward the port over SSH.

## Privacy

- **Local first.** Captures and previews stay in `~/.slink` until you publish.
- **End-to-end encrypted.** Each share is encrypted on your machine with a fresh
  AES-256-GCM key carried in the link's `#` fragment, which browsers never send
  to the server. Named shares open only for the verified emails you invite.
- **No API keys in captures.** The recorder drops auth headers.
- **Scanned before upload.** Exports are checked for common credential formats.
  This is a safety net, not data-loss prevention.
- **Recoverable.** A recovery key and approved devices restore your share links;
  email or GitHub sign-in alone cannot.

Links published by v0.5.0 and earlier (`/r/…`) are unlisted but not encrypted.
Details: [identity and encryption](docs/identity-encryption.md),
[named-recipient sharing](docs/named-recipient-sharing.md).

## What's here

| Path | Contents |
| --- | --- |
| [`go/`](go) | The `slink` CLI and always-on recorder: one native binary. |
| [`packages/format`](packages/format) | `@session-link/format`: the open `session/v0` format, schema and validator. |
| [`packages/viewer`](packages/viewer) | `@session-link/viewer`: the reader embedded in the CLI and served by session.link. |
| [`packages/pi-extension`](packages/pi-extension) | `/slink` for pi. |
| [`docs/`](docs/index.md) | Design and protocol notes. |

The format and viewer are MIT-licensed, so a `session/v0` file stays readable
with or without the hosted service; `--server` or `SLINK_SERVER` points the CLI
at any compatible host. The service at [session.link](https://session.link) is
a separate repository.

## Develop

```bash
npm ci && npm run build:viewer     # the Go binary embeds the viewer bundle
cd go && go test ./... && go build -o slink ./cmd/slink
npm test                           # format, viewer and pi extension
npm run dev:viewer                 # the viewer with fixtures at http://127.0.0.1:4173
```

Issues and patches are welcome.

## License

MIT. Also on [Radicle](https://radicle.xyz): `rad clone rad:z24FnLsshNV8kq5fWCTi2dbKueomr`.
