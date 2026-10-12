# Compatibility baseline for the Rust migration

2026-10-11, revised 2026-10-12 · P0 reference record for the [Rust migration](rust-migration.md) · inventory and differential runner implemented; coverage gaps and unmeasured baselines are listed below.

This is the record the port is held to: what the Go CLI and the hosted service
expose today, how a candidate implementation is compared with the reference,
what the comparison found, and what it does not reach yet. It is the first item
of the plan's implementation backlog.

Nobody uses the software yet, so the port is held to the product's behavior and
not to state the Go CLI or the Node service wrote; the plan's
[Starting fresh](rust-migration.md#starting-fresh) section has the decision.
The Go binary is the oracle because it is the cheapest proof that the port does
the same thing, and a deliberate difference is declared here.

## Reference

| What | Pinned at |
| --- | --- |
| Client | `fb2da26` on `main`, released as v0.9.1 |
| Hosted service | `2250b3b` in `session-link-server`; the [architecture review](architecture-review.md) found Fly release 58 running |
| Viewer bundle | SHA-256 `eec340330565f26779368b73b2e226a2dabdf786572e88b5b8659c20ef22942e`, the same for the bundle the CLI embeds and the one checked into the server |
| Changed since, on purpose | Two protocol rules of 2026-10-12, in Go, the viewer and the service together: no signing key of small order, and device names of whole characters only ([rust-protocol.md](rust-protocol.md), findings seven and eight). The bundle with them is `948b0ad37fdf05832db038c1d1b9b243ef1f6ece5acc6f2378658b5299305a3c` |
| Packages | `@session-link/format` 0.1.1, `@session-link/viewer` 0.7.1, `@session-link/pi-extension` 0.1.0 |
| Harness versions | The matrix in [handoff-design.md](handoff-design.md#harness-contract-and-current-verification) |

## Inventory

[compat-inventory.mjs](../scripts/compat-inventory.mjs) reads the surface from
the sources and writes it under `testdata/compat/`. With `--check` it fails on
any drift, and CI runs that, so a command, flag or variable cannot change in Go
without the contract changing in the same commit.

- [cli.json](../testdata/compat/cli.json) holds the 17 commands and their 3
  aliases, each command's flags with type and default, whether it touches local
  data, the spellings of help and version, and the 25 environment variables the
  Go source names. The script also fails when the `commands` list in `help.go`
  and the switch in `dispatch()` disagree.
- [hosted.json](../testdata/compat/hosted.json) holds the 31 routes and pages
  with their methods, the 3 rewrites and the 61 static files of the hosted
  service, at the server revision it records. The server is another repository,
  so this file is only read or written when `--server` names a checkout.

Two parts of the contract are not data yet. Exit codes follow a convention the
runner asserts wherever a case reaches it: 0 for success, 1 for a failure or a
declined confirmation, 2 for a usage error, and for `record` the child's own
code, 127 when the command is not in `PATH`, 126 when it cannot start and 128
plus the signal number when a signal killed it
([devopen.go](../go/cmd/slink/devopen.go)). The hosted service's observable
responses are recorded only as the checks in the server's `scripts/smoke.mjs`
and `scripts/smoke-encrypted.mjs`.

## Differential runner

[compat.mjs](../scripts/compat.mjs) runs each case through a reference
executable and a candidate, each in its own throwaway home, and compares what a
user or a script can observe: the exit code, stdout, stderr, and the files left
in the slink home with their modes. `SLINK_BINARY` names the reference and
`SLINK_CANDIDATE` the candidate.

| Comparison | Meaning | Used for |
| --- | --- | --- |
| bytes | Identical after normalization | The default: machine output, error messages, capture documents, file modes |
| json | Equal as parsed JSON; fields a case names are compared by presence and type | `list --json`, recorded documents |
| behavior | The exit code, which stream is used, and the patterns the case names | Help text, whose spacing may differ, and messages the flag parser writes |

Comparisons are strict by default. A case that needs a wider one says so where
it is defined, with the reason; the runner's normalization is not the place to
make a difference disappear. Normalization rewrites only values the runner
knows were generated:

- the throwaway root;
- the stamp and ID of each capture created during the case, numbered in the
  order the steps created them;
- loopback ports;
- relative ages such as "3d ago" with their column padding, which move with
  the clock.

Recording cases also compare `created_at`, `started_at` and `ended_at` by
presence and type, since they are the recorder's own clock.

Without a candidate the reference is compared with a second run of itself.
That is how the list above was arrived at, and it is what "the current binary
passes" means until a second implementation exists: all 104 cases agree for the
Go binary at the pinned revision on macOS arm64, in three consecutive runs. A
deliberately wrong candidate (changed stdout, output on the wrong stream, a
different exit code, a changed JSON field, a looser file mode) was caught in
every one of those ways.

The first real candidate is `slink-rs`, the binary of the
[Rust workspace](rust-build.md). It has `version` and nothing else: the four
version cases agree, and every other case fails with its "not ported yet"
message, which is the runner reporting the port's progress as intended.

Nothing in a run reaches the network or the real `~/.slink`. `HOME` and
`SLINK_HOME` point into the throwaway root, `PATH` is an empty directory so
there is no clipboard or browser, and the only upstream is a replay server on
loopback.

## What the cases cover

| Group | Cases | Covers |
| --- | --- | --- |
| usage | 11 | No command, every spelling of help and version, unknown commands with and without a suggestion |
| help | 37 | `-h` for every command and alias, and `help <command>`: exit 0, the right stream, every flag in the inventory named |
| flags | 2 | A value flag with no value and an unknown flag exit 2 |
| completion | 4 | The bash, zsh and fish scripts byte for byte; no shell named |
| shell | 2 | `off` in a clean shell; `on --no-start` with no recorder running |
| local | 1 | An empty home: both listings, `prune`, `push` and `import` with nothing to act on |
| import | 32 | Every fixture a path can name for Claude Code, Codex, pi, omp, Aider and DeepSeek Harness: the document's bytes, file modes, reuse on a second import, both listings; recognition without `--from`; flags after the transcript |
| discover | 1 | A Claude Code store: the project found from a subdirectory, a pasted session ID, the hint when a directory has no session, `share` stopping without an account |
| record | 14 | Ten recorded provider exchanges through `slink record` against a replay upstream: the response reaches the child unchanged and the recorded document agrees; the child's exit code; 126 and 127; an argument before `--` |

## How the fixtures are classified

| Fixtures | Compared as | Reason |
| --- | --- | --- |
| `testdata/import` through `slink import` | Bytes of the capture document | Go writes identical bytes on every import, and reuse of an unchanged capture and the hosted dedupe both depend on an implementation agreeing with itself. Agreeing with Go is the strictest proof an importer is right, not a requirement |
| `testdata/proxy` through `slink record` | JSON, with the three time fields by type | Only the recorder's clock is generated; the raw request and response must agree |
| Listings | `list --json` as JSON, `list` rows as bytes | The JSON is the scripting surface; the rows carry the ID every command accepts |
| Error messages | Bytes | Strict until a case states otherwise |
| Help text | Behavior | The plan lets incidental spacing differ |
| `testdata/format` | Verdict, where the error is and the scan's findings, in the JavaScript, Go and Rust tests | The validators and scanners are libraries, so each language's tests read the cases; [rust-format.md](rust-format.md) has the differences they record |
| `testdata/spool` | Not in the runner; Go tests in process | Needs spools placed in a home and a reader run over them |
| `testdata/encryption` | Not in the runner; envelopes sealed by Web Crypto and by Go, opened in the Go, JS and Rust tests | The two recorded vectors the envelope cases in `testdata/protocol` are derived from |
| `testdata/protocol` | Verdict, and the state read where a history is accepted, in the JavaScript, Go and Rust tests | The verifiers are libraries, so each language's tests read the cases; [rust-protocol.md](rust-protocol.md) has the differences they record |
| `testdata/share/research` | Not in the runner; Go tests and the browser checks | The export is driven through the local server |
| `testdata/viewer/arrival` | Behavior, through the browser check scripts | The viewer is not being ported |

## Found while building it

They are recorded as the Go binary behaves, and Go is not changed. With nothing
to stay compatible with, each has an answer for the port; the runner's case
changes to declare the difference when the Rust code for it lands.

1. `slink record -h`, `slink dev -h` and `slink help record` write their help
   to stderr. Every other command writes it to stdout. Without `--`, `record`
   hands its arguments to the flag package directly. The port writes this help
   to stdout like the rest.
2. Captures list by file name, and the stamp in the name has one-second
   resolution, so two captures made within a second list in the order of their
   random IDs. `push` and `share` with no argument take the first as the
   newest. The runner waits for the next second between imports. The port makes
   "newest" well defined; whether by a finer stamp or by reading `created_at`
   is settled when `list` is ported.
3. A recorded document's `source.label` is `slink-go@<version>`. The Rust
   recorder writes `slink-rs@<version>`, and the recording cases will compare
   that field by its shape.
4. Imported documents are byte-identical from Go because it sorts object keys
   and escapes `<`, `>` and `&`. The port must write the same bytes for the
   same input every time, but they need not be Go's: no capture Go wrote has
   to be reused. The import cases stay on bytes while that holds for the port,
   and a family moves to the JSON comparison, with the reason here, when its
   serialization differs on purpose.

## Not covered yet

- opencode and Hermes imports. Their fixtures are rows, so the runner needs
  SQLite stores built from them; until then two of the eight harnesses are held
  only by Go's in-process golden tests.
- Discovery for the harnesses other than Claude Code, resumed branches,
  ambiguous sessions, and `view` with its picker.
- The local server: the `view` routes and access capability, background mode,
  drafts, saved views and the excerpt export.
- Publication, login, identity, recovery and named sharing, which need the
  server's smoke environment.
- The always-on recorder: `tap`, `on` starting it, `setup`, `status`, `doctor`,
  `prune` with captures present, and the login service.
- Spool recovery and interoperation, signals, concurrent calls and a full disk.
- The fixtures the plan asks for as each area is ported: unfinished identity
  setup, pending ciphertext, named outboxes, torn spools and rejected
  histories. Homes written by earlier versions are out of scope.
- Linux and Windows. The runner has run only on macOS arm64, so it is not in CI.
- The hosted service's responses as data.

## Baseline measurements

Measured on the pinned client revision, on an Apple M1 Max with macOS 26.6.2
and Go 1.27.1. CI builds with Go 1.26.

| Measurement | Result |
| --- | --- |
| Binary built with the release flags, darwin/arm64 | 13.2 MiB |
| `slink version`, 20 runs after 5 warm-ups, including process start | 13.7 ms median, 13.9 ms P95 |

Not measured yet: download sizes for v0.9.1, the recorder's idle and active
memory, streaming overhead, local first render, and hosted upload and download
latency. The numeric budgets P0 calls for are not set.
[kpis.md](kpis.md) defines how each is to be measured.

## P0 status

| P0 item in the plan | State |
| --- | --- |
| Reference revisions, versions, viewer digest, harness matrix | Done, above |
| Command, variable and hosted contracts | CLI and hosted routes done; hosted responses outstanding |
| Differential runner with classified comparisons | Built; coverage as listed |
| Extended fixtures | Not started; they arrive with the area they test |
| Baseline measurements | Two of the listed measurements |
| Recovery objectives and a rehearsed restore | Left P0 on 2026-10-12: the current volume is not restored or migrated. The plan's P7 rehearses a restore of the new store |
