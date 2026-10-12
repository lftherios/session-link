# Rust CLI shell, first importer and local reader

2026-10-12 · Implemented slice of the [Rust migration](rust-migration.md), backlog item 6 · `slink-rs` knows every command, imports Claude Code sessions, lists captures and serves one in the local viewer; 63 of the 104 runner cases agree with Go; run locally on macOS arm64, not yet in CI.

The first journey the Rust binary does whole, in a home of its own: a Claude
Code session is imported, shows in the listing, and is read in a browser from
a server on loopback. Nothing in this binary publishes. The commands that
would say so and stop, and so does every endpoint of the local server that
the Go viewer uses to reach an account or the hosted service.

## What works

| Command | State |
| --- | --- |
| `version`, `help`, `-h` on every command | As Go, byte for byte |
| A wrong flag, an unknown command with its suggestion | As Go |
| `completion bash\|zsh\|fish` | As Go, byte for byte |
| `list`, `ls`, `list --json`, `--limit` | Captures in the home. Spool recovery arrives with the recorder |
| `import` | Claude Code: a transcript by path, `--session`, a pasted session id, or the newest for this directory and its parents, with its sub-agents. The other seven harnesses are named and refused |
| `view`, `open` | A capture by id, a `session.json`, or a Claude Code transcript or session id; `--port`, `--no-browser`, `--span`, `--from`, `--pick`. `--background` is not ported |
| `share`, `push`, `login`, `logout`, `delete` | Refused: "cannot publish yet", exit 1, nothing written or sent |
| `record`, `tap`, `setup`, `on`, `off`, `status`, `doctor`, `prune` | Parsed, then "not ported yet", exit 1 |

## Layout

| Crate or file | What it holds |
| --- | --- |
| [crates/slink-local](../crates/slink-local) | The slink home: where it is, private and atomic file writes, the captures in it, saved previews |
| [crates/slink-import](../crates/slink-import) | The importers and the code that finds a session on disk. Claude Code is the one harness so far |
| [slink-format](../crates/slink-format) | Gained the document writer, a white-space compactor and a timestamp reader |
| [commands.rs](../crates/slink/src/commands.rs) | The 17 commands with their flags and help text, the usage text, suggestions and completion scripts |
| [flags.rs](../crates/slink/src/flags.rs) | Flags read as Go's `flag` package reads them, and help printed as it prints it |
| [import.rs](../crates/slink/src/import.rs), [list.rs](../crates/slink/src/list.rs) | The two commands, with Go's messages |
| [view.rs](../crates/slink/src/view.rs) | The view command and its loopback server |

The two new crates are the plan's, created now that their first code exists.
Functions that need the time or random bytes are handed them; the binary
takes both from the operating system.

## Choices

| Area | Choice | Reason |
| --- | --- | --- |
| Command surface | A table in the binary, not a parser library | The surface is fixed by `testdata/compat/cli.json`, and Go's flag grammar is unusual: one dash or two, flags after arguments, help text of its own shape. A library would have to be bent to each of those |
| Document bytes | What Go's `encoding/json` writes | An import is found again by its bytes and the service dedupes by them. Go's output is the oracle the runner compares with |
| Numbers | `float_roundtrip` in serde_json | See the first finding |
| Home layout | Go's: `runs/<stamp>-<id>.json`, `previews/<sha256>.json`, directories 0700 and files 0600 | Nothing asked for another, and the runner compares the files a command leaves and their modes |
| Local access | Go's model unchanged: loopback only, the printed host name only, a key for each run in the URL's fragment, sent by the page as a header | It is what the security check script tests, and the access script is read from the Go tree and not copied |
| Publishing | Absent, not stubbed to succeed | The plan's rule: nothing leaves the machine without the publish action, and there is no half of publishing that is safe to have |

## The importer

[claude_code.rs](../crates/slink-import/src/claude_code.rs) is the Go
importer line for line in meaning. Three checks hold it:

- The six fixtures under `testdata/import/claude-code` reproduce their golden
  runs as parsed JSON and are valid `session/v0`, in the crate's own test.
  This is the test the Go importers pass.
- Through the differential runner, the capture each fixture leaves is the
  same bytes Go leaves, in a file of the same mode. Importing it again finds
  the first capture and writes no second.
- Discovery agrees in the runner's Claude Code store: the project found from
  a subdirectory, a pasted session id, and the hint when a directory has no
  session.

## The local reader

`view` saves the session it was asked for under the SHA-256 of its bytes and
serves that copy, so an agent still working cannot change a page someone is
reading. A session whose tool results outweigh the rest by 512 KiB also gets
a reading copy without them, as in Go.

The existing [check-viewer-browser.mjs](../scripts/check-viewer-browser.mjs)
drives whatever binary it is given. Against `slink-rs` its first four groups
pass in headless Chromium: the latest input and answer, search, navigation
and links, and reading position. It stops at the fifth, composing a view to
share, which needs endpoints this slice does not have. Against Go it passes
whole.

What the server does not have yet:

- The Go picker at `/`, with search and days. `slink-rs` lists the sessions
  it found as plain rows. With no reference it offers the newest Claude Code
  session for the project and the captures recorded there, not the thirty
  most recent across harnesses.
- Typed titles, composing and exporting a view, drafts, `/r/<id>` for a live
  capture, the recovery and shared pages, and every publish, login, identity
  and named-share endpoint. Each answers 501 with one message.
- Reuse of a preview by its source files' size and time. Every `view`
  imports again and finds the same preview by its hash.

## Evidence

| Check | Result |
| --- | --- |
| `SLINK_BINARY=go/slink SLINK_CANDIDATE=target/release/slink-rs node scripts/compat.mjs` | 63 of 104: usage 11 of 11, help 37 of 37, flags 2 of 2, completion 4 of 4, import 9 of 32 |
| The same with no candidate | 104 of 104, as before |
| `help` for all 17 commands against Go's | Identical bytes |
| [tests/cli.rs](../crates/slink/tests/cli.rs) | The journey in a home of its own: import, the capture's mode, reuse, both listings, the page with and without the key, another host name, a foreign origin, stop |
| The viewer browser check against `slink-rs` | Four of its groups, as above |

The 41 cases that differ are the commands not ported: the other five
file-based importers, `record`, `prune`, `on` and `off`, and the `push` and
`share` steps inside two cases whose other steps agree.

One difference is declared in the runner. `slink record -h` writes its help
to stderr in Go and to stdout here, which the
[baseline](compatibility-baseline.md) had decided.

## What building it found

1. **serde_json does not read every number exactly.** By default it reads
   some long decimals one step away from the nearest double, so
   `123456789012345680000` came back as `123456789012345670000`. A recorder
   that changes a digit is not a recorder. The `float_roundtrip` feature
   reads them exactly and is on for the whole build.
2. **Go changes integers past 2^53.** It reads every number as a double, so
   such an integer in a transcript is written back as a different one. Rust
   keeps it. No fixture holds one, so the bytes agree today; on a transcript
   that does, they would differ and Rust would be the faithful side.
3. **A line that is not UTF-8.** Go replaces each bad byte with U+FFFD and
   Rust each bad sequence, which can be one character where Go writes two.
   No fixture holds one.
4. **A model id that is not text.** Go prints its own debug form of an object
   there and Rust prints JSON. Neither is reachable from a real transcript.

## Cost

Measured on macOS arm64 with the release profile.

| Build | Binary |
| --- | --- |
| Before this slice | 7,030,032 bytes |
| With the shell, the importer and the reader | 7,528,032 bytes |

The lockfile holds 217 packages, from 213: the two new crates, and `errno`
and `signal-hook-registry`, which Tokio needs to hear a termination signal.

## Not done

- The other seven importers, and discovery for them.
- `view --background`, the picker page, titles, composing and exporting.
- Spool recovery in listings, which belongs with the recorder.
- `prune`, `on`, `off`, `status`, `doctor`, `setup`, `tap`, `record`.
- Publication and identity, which are backlog item 8.
- The runner in CI, and on Linux and Windows at all. `cargo test` runs the
  journey on every target once this is pushed.
- The security browser check against `slink-rs`: it begins by importing a pi
  session, which this binary cannot do yet.
