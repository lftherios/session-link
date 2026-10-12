# Rust CLI: importers, the local viewer and excerpts

2026-10-12 · Implemented slices of the [Rust migration](rust-migration.md), backlog items 6 and 7 · `slink-rs` imports all eight harnesses, serves the local viewer with its sessions page, and composes and exports excerpts; 86 of the 104 runner cases agree with Go, and both browser checks pass against it. The tests of both pass on all six targets in CI.

Everything a person does with a session before publishing it now runs in the
Rust binary, in a home of its own: a session of any supported agent is found
and imported, read in a browser from a server on loopback, and cut down to an
excerpt that leaves out what was not chosen. Nothing in this binary
publishes. The commands that would say so and stop, and so does every
endpoint of the local server that the Go viewer uses to reach an account or
the hosted service.

## What works

| Command | State |
| --- | --- |
| `version`, `help`, `-h` on every command | As Go, byte for byte |
| A wrong flag, an unknown command with its suggestion | As Go |
| `completion bash\|zsh\|fish` | As Go, byte for byte |
| `list`, `ls`, `list --json`, `--limit` | Captures in the home. A capture still being recorded is not listed; see "Not done" |
| `import` | All eight harnesses: a transcript or history by path, `--session` with a path or an id, a pasted session id, or the newest session for this directory and its parents, with its sub-agents. Hermes says it is experimental, as in Go |
| `view`, `open` | A capture by id, a `session.json`, or a session of any harness by path or id; the sessions page when nothing is named, or the session the command was run from when DeepSeek Harness says which; `--port`, `--no-browser`, `--span`, `--from`, `--pick`, `--background` |
| `share`, `push`, `login`, `logout`, `delete` | Refused: "cannot publish yet", exit 1, nothing written or sent |
| `record`, `tap`, `setup`, `on`, `off`, `status`, `doctor`, `prune` | Parsed, then "not ported yet", exit 1 |

## Layout

| Crate or file | What it holds |
| --- | --- |
| [crates/slink-import](../crates/slink-import) | One module for each harness, holding its importer and how its sessions are found: [claude_code.rs](../crates/slink-import/src/claude_code.rs), [codex.rs](../crates/slink-import/src/codex.rs), [pi.rs](../crates/slink-import/src/pi.rs) for pi and omp, [dsh.rs](../crates/slink-import/src/dsh.rs), [aider.rs](../crates/slink-import/src/aider.rs), [opencode.rs](../crates/slink-import/src/opencode.rs) and [hermes.rs](../crates/slink-import/src/hermes.rs) |
| [discover.rs](../crates/slink-import/src/discover.rs) | What the harnesses share: the newest session for a directory, a session by id, the listing the sessions page shows, reading a transcript, and telling which harness wrote a file |
| [value.rs](../crates/slink-import/src/value.rs), [images.rs](../crates/slink-import/src/images.rs), [db.rs](../crates/slink-import/src/db.rs) | JSON read as the JavaScript the importers began as read it; image parts; a SQLite store opened read-only |
| [crates/slink-local](../crates/slink-local) | The slink home: where it is, private and atomic file writes, captures, saved previews and when one is used again, [drafts.rs](../crates/slink-local/src/drafts.rs) for drafts, typed titles and saved views |
| [share](../crates/slink-local/src/share) | The excerpt: [catalog.rs](../crates/slink-local/src/share/catalog.rs) says what of a session can be chosen, [export.rs](../crates/slink-local/src/share/export.rs) builds the document that holds only that |
| [slink-format](../crates/slink-format) | Gained the document writer, an indented writer for excerpts, a white-space compactor and a timestamp reader |
| [commands.rs](../crates/slink/src/commands.rs), [flags.rs](../crates/slink/src/flags.rs) | The 17 commands with their flags and help text, read and printed as Go's `flag` package does |
| [import.rs](../crates/slink/src/import.rs), [list.rs](../crates/slink/src/list.rs) | The two commands, with Go's messages |
| [view.rs](../crates/slink/src/view.rs) | The view command: which sessions it offers, the background process, the browser |
| [server.rs](../crates/slink/src/server.rs), [picker.rs](../crates/slink/src/picker.rs) | The loopback server and the sessions page. The page's styles and scripts are Go's, copied into [assets](../crates/slink/src/assets) |

Functions that need the time or random bytes are handed them; the binary
takes both from the operating system.

## Choices

| Area | Choice | Reason |
| --- | --- | --- |
| Command surface | A table in the binary, not a parser library | The surface is fixed by `testdata/compat/cli.json`, and Go's flag grammar is unusual: one dash or two, flags after arguments, help text of its own shape. A library would have to be bent to each of those |
| Document bytes | What Go's `encoding/json` writes | An import is found again by its bytes and the service dedupes by them. Go's output is the oracle the runner compares with |
| Numbers | `float_roundtrip` in serde_json | See the first finding |
| Home layout | Go's: `runs/<stamp>-<id>.json`, `previews/<sha256>.json`, `drafts/`, directories 0700 and files 0600 | Nothing asked for another, and the runner compares the files a command leaves and their modes |
| Local access | Go's model unchanged: loopback only, the printed host name only, a key for each run in the URL's fragment, sent by the page as a header | It is what the security check script tests, and the access script is read from the Go tree and not copied |
| Publishing | Absent, not stubbed to succeed | The plan's rule: nothing leaves the machine without the publish action, and there is no half of publishing that is safe to have |
| Importers | Go's code line for line in meaning, its odd corners included | Bytes equal to Go's are the cheapest proof a port is right. What looked wrong in Go is listed below and left for a decision, not fixed in passing |
| Databases | rusqlite with SQLite built in, opened read-only | The plan's choice, already proved on six targets. opencode and Hermes keep their sessions in one |
| Local time | chrono | Aider writes local times with no zone, and the sessions page groups by local day. For a time that a clock change skipped or repeated the rule is Go's, pinned by a test with made-up zones |
| A preview used again | Found by harness, session id and files; valid while the files' sizes and times and the binary are unchanged | See the fifth finding: files alone are not a session |

## The importers

Each is held three ways.

- **Goldens.** The 33 fixtures under `testdata/import` reproduce their golden
  runs as parsed JSON and are valid `session/v0`, in the crate's tests
  through one helper, [tests/common](../crates/slink-import/tests/common/mod.rs).
  This is the test the Go importers pass.
- **Bytes.** Through the differential runner, the capture each file-based
  fixture leaves is the same bytes Go leaves, in a file of the same mode, and
  importing it again writes no second. opencode and Hermes fixtures are rows,
  which the runner cannot place; their tests write the rows to a SQLite
  database and read it back through the same loader the command uses.
- **Discovery.** The Go discovery tests are ported for each harness: the
  session for a directory and from a subdirectory, by id, a sub-agent's
  session not offered as the person's own, ranking by last activity, a store
  that cannot be read, and where each harness keeps its store.

| Harness | Fixtures | Runner cases | Compared with Go beyond the runner, while porting |
| --- | --- | --- | --- |
| Claude Code | 6 | 7, and the discovery case's five local steps | |
| Codex | 5 | 6 | 56 hand-written rollouts and 1,000 random ones, 23 discovery commands, a 30-rollout store listed 23 ways |
| pi and omp | 4 and 3 | 5 and 4 | 58 hand-built transcripts each read three ways, 1,000 random ones, nine discovery scenarios of about 80 steps |
| DeepSeek Harness | 4 | 5 | 84 importer cases and 19 of discovery, of which six differ and are listed below or were pi not yet being in that copy; 53 compressed logs cut and damaged, all alike; the 1 GiB limit from both sides |
| Aider | 2 | 3 | 351 cases, among them 11 zones at 24 clock times; 341 alike, the rest are the differences below |
| opencode | 5 | none | 428 outputs: fixtures from rows and from databases, 352 generated sessions, malformed rows, discovery on five databases |
| Hermes | 4 | none | 248 importer cases, 279 lines of discovery and loading over nine databases, 26 store locations |

The last column is what each port ran once against the Go binary with
generated input. Those scripts were scratch work and are not in the
repository; what stays is the goldens, the ported tests and the runner.

## The local reader

`view` saves the session it was asked for under the SHA-256 of its bytes and
serves that copy, so an agent still working cannot change a page someone is
reading. A session whose tool results outweigh the rest by 512 KiB also gets
a reading copy without them, as in Go.

- **Sessions page.** With nothing named, `/` is Go's page: the thirty most
  recent sessions for the project across every harness and the captures
  recorded there, with search, days and titles.
- **Background.** `--background` starts the viewer as a process of its own,
  prints the address once it listens and returns. Its diagnostics go to
  `viewer-<pid>-<time>.log` in the home. It is stopped from the sessions page.
  Neither it nor whatever opens the browser is given the command's own
  streams, so a caller that reads them to the end is done when the command
  is; the eighth finding has what that took on Windows.
- **A preview used again.** A session whose files and binary have not changed
  is not imported again.
- **Excerpts.** `/compose/<id>` and the endpoints behind it: the catalog of
  what can be chosen, drafts saved by revision so that a second tab cannot
  overwrite the first, typed titles, saved views, and the export. The React
  components are the ones Go serves.
- **What is left out stays out.** The export is built from an allowlist: a
  field is in the excerpt because the exporter put it there. The research
  fixture exports to exactly [excerpt.json](../testdata/share/research/excerpt.json),
  and its marker `OMITTED_INTERNAL_STRATEGY` is absent from the stored
  excerpt, its page, and what `/api/document` returns for it.

## Evidence

| Check | Result |
| --- | --- |
| `SLINK_BINARY=go/slink SLINK_CANDIDATE=target/release/slink-rs node scripts/compat.mjs` | 86 of 104: usage 11 of 11, help 37 of 37, flags 2 of 2, completion 4 of 4, import 32 of 32 |
| The same with no candidate | 104 of 104, as before |
| `cargo test` | 211 pass, 114 of them in the importer crate, 23 in the home crate and 13 that run the built binary, one of those on Unix only |
| [check-viewer-browser.mjs](../scripts/check-viewer-browser.mjs) against `slink-rs` | All 11 groups, composing and exporting a view among them |
| [check-security-browser.mjs](../scripts/check-security-browser.mjs) against `slink-rs` | Passes: private capture modes, reads and actions only with the key, a reload, a copied link, framing refused, no image fetched until asked. Six runs of six, and six of six against Go |
| [tests/cli.rs](../crates/slink/tests/cli.rs) | The journeys in homes of their own: import to page to stop; an excerpt composed, saved, exported and reopened; a background viewer; a browser opener that stays; two Aider runs as two previews; the pi extension's command; a view opened from inside a session. A test that fails stops the viewer it started |
| [pi-handoff.test.mjs](../test/pi-handoff.test.mjs) with `SLINK_HANDOFF_BINARY` | The real extension's `/slink view` gets its address from the built binary, Rust or Go, and reports a session that is not there |

The 18 cases that differ are the commands not ported: `record`, `on` and
`off`, and the `prune`, `push` and `share` steps inside two cases whose other
steps agree.

One difference is declared in the runner. `slink record -h` writes its help
to stderr in Go and to stdout here, which the
[baseline](compatibility-baseline.md) had decided.

The security check needed one change to pass reliably against either binary.
It asked the page a question while the page was still arriving, and failed
about one run in two against Go as well. It now asks again.

## Where the Rust importers differ from Go

Each is known and none shows on a fixture.

| Where | Go | Rust |
| --- | --- | --- |
| A whole number past 2^53 that passes through untouched | Changed to the nearest double | Kept. Where Go makes text of such a number, as for an id, Rust makes the same text |
| JSON nested 128 levels or deeper inside a line or a column | Read | Refused by the format crate, so the line is skipped or the text stays text |
| Text that is not UTF-8 in a database column | One U+FFFD for each bad byte | One for each bad sequence. Transcripts and histories read as Go reads them |
| `TZ` that names no zone file, for Aider's times | UTC | chrono's reading: a POSIX rule is honoured, an unknown name is the system zone |
| A database that fails part of the way through a listing | The sessions read so far, and the problem | The problem only |
| What follows `cannot read … sessions:` | The Go driver's words | SQLite's own |
| An id with unprintable characters in a not-found message | Escaped as `%q` escapes | Escaped for quotes, backslash and control characters only |
| `..` in a store path taken from the environment | Cleaned away | Kept as written |
| The working directory when reached through a symlink | `$PWD` | The resolved path |
| The Hermes database, between finding a session and loading it | Looked for again | The file it was found in |
| A compressed log damaged in a way other than being cut short or failing its checksum | The Go decoder's words; a frame's stated size is checked against its content | The Rust decoder's words; the size is not checked, and a single-segment frame above 512 MiB is refused |
| Two DeepSeek Harness sessions that each name the other as a sub-agent | Any listing recurses until the stack is gone | The listing stops where the import stops, nine deep. omp's listing has the same end |

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
3. **A line that is not UTF-8.** Go replaces each bad byte with U+FFFD.
   Rust's standard conversion replaces each bad sequence, which can be one
   character where Go writes two. Transcripts are now read by a conversion
   of our own that follows Go, [lossy](../crates/slink-import/src/discover.rs),
   with a test. No fixture holds such a line.
4. **A model id that is not text.** Go prints its own debug form of an object
   there and Rust prints JSON. Neither is reachable from a real transcript.
5. **Go can show the wrong Aider run.** It finds a saved preview again by
   harness and files. Every run in an Aider history is in one file, so
   `slink view` of a second run, with the file unchanged, serves the first
   run's preview: both commands print the same address. Rust finds a preview
   by the session's id as well, and
   `two_runs_in_one_aider_history_are_two_previews` fails without it. Go is
   not changed.
6. **The oracle's bytes depend on its toolchain.** Built with Go 1.27, whose
   JSON encoder is new, the reference writes an invalid byte as a raw U+FFFD.
   Built with Go 1.26, which CI and releases pin, it writes the six
   characters of the escape. Only a transcript that is not UTF-8 shows it,
   and no fixture is one, but a local run and a CI run of the runner can
   disagree there.
7. **Image sniffing read base64 its own way.** An unlabelled image is
   recognised by its first bytes. The first Rust version decoded a partial
   group and stopped at a line break; Go skips line breaks and decodes whole
   groups of four. A short string became an image in one and stayed data in
   the other. Rust now follows Go's rule, and a pi test pins it.
8. **A background viewer kept its caller waiting.** Windows gives a new
   process every handle of its parent's that may be inherited, and the
   streams a command is started with are such handles. Go's runtime names
   the handles a child gets; Rust's standard library does not. The viewer
   started by `--background` therefore held the caller's pipes for as long
   as it ran, and the first run of these tests on Windows waited three and a
   half hours on two of them before it was cancelled. The command now marks
   its own streams as not to be inherited before it starts the viewer,
   [keep_from_children](../crates/slink/src/view.rs), which is one call to
   the system and the only unsafe code in the workspace: the CLI crate
   denies unsafe code where the others forbid it, and allows that function.
   The same wait was possible on every system through the browser's opener,
   which Go starts with no streams and Rust started with the command's. It
   has none now, and `a_browser_that_stays_open_does_not_hold_the_caller`
   takes twenty seconds and fails without that. The build jobs in CI stop
   after thirty minutes.

### Left as Go has it

The ports reproduce these and none is fixed. Each is a decision about the
product and not about the port.

| Harness | What Go does |
| --- | --- |
| Codex | A sessions directory that is itself a symlink finds nothing, because the walk does not follow its root |
| Codex | `slink import rollout-x.jsonl` from inside the day's directory looks for sub-agents in that directory only; an absolute path searches the store |
| Codex | Token counts land on the response before the one they belong to, and the last response has none. The sub-agent fixture's golden shows it |
| Codex | Two calls with one id: the first result closes the later call, the earlier never closes |
| Codex | Only the 500 newest rollouts are opened when looking for the latest session |
| opencode, Hermes | The "newest session elsewhere" hint can name a sub-agent's session, and ranks by when a session began where everything else ranks by last activity |
| opencode | A time past the year 2262 overflows to a negative recency |
| Hermes | A session ranks by its newest message even when that message was rewound and is not imported |
| Hermes | A store that lacks only the unused `tool_name` column cannot be read |
| Hermes | A time outside years 0000 to 9999 is written with five digits or a sign, which RFC 3339 does not allow |
| Hermes | A sub-agent's session that fails to load is left out without a word |
| pi, omp | A transcript with no timestamp anywhere yields a document with no `created_at`, which the schema requires. It is written anyway and listed with `?` |
| omp | The import and the sessions page can name one session differently: the import takes the last title change even when it is empty, the page the last that is not |
| omp | `PI_CODING_AGENT_DIR` has `~` expanded for pi and not for omp |
| Aider | Two runs begun in the same second share an id, and only the later can be opened by it |
| Aider | A history recognised by its first line and not its name looks for its input history under a name Aider never writes |
| DeepSeek Harness | A log named by a bare relative path, as `slink import session.v4.jsonl` from its own directory, loses its sub-agents and images and can be named `.` |
| DeepSeek Harness | A log is taken for compressed by its `.zstd` name when it is looked for and by its first bytes when it is read, so a file that is one and not the other imports by path and is never found |
| DeepSeek Harness | A header saying version 4.9 is read as version 4 |
| DeepSeek Harness | `"error": false` on a tool result marks it failed, as any value but null does |

## Cost

Measured on macOS arm64 with the release profile.

| Build | Binary |
| --- | --- |
| Before the shell | 7,030,032 bytes |
| With the shell, the Claude Code importer and the reader | 7,528,032 bytes |
| With all eight importers, the sessions page and excerpts | 8,454,400 bytes |

The lockfile holds 228 packages, from 217. chrono gives local time to
Aider's histories and the sessions page, and brings what it needs to find the
system's zone on each platform.
ruzstd reads the compressed logs of DeepSeek Harness. rusqlite was already
there for the build check and now has callers.

## Not done

- **Captures still being recorded.** The plan's P2 asks that a live capture
  be read without taking over the recorder's spool. Reading one is the spool
  engine's snapshot, with its lock and its test of whether the owner is
  alive, and no Rust recorder writes a spool yet. It is left to the recorder
  slice, where both sides are written against the
  [protocol](spool-protocol.md) together. Until then `list` shows finished
  captures only and the server has no `/r/<id>`.
- `prune`, `on`, `off`, `status`, `doctor`, `setup`, `tap`, `record`.
- Publication and identity, which are backlog item 8. The server's publish,
  login, identity and named-share endpoints answer 501 with one message, and
  it has no `/settings` or `/shared` page. Asked whether anyone is signed in,
  it says no.
- The runner in CI, and on Linux and Windows at all.
- The browser checks and the opt-in handoff test in CI. They were run by
  hand.
