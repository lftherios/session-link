# Rust format validation and credential scan

2026-10-12 · Implemented slice of the [Rust migration](rust-migration.md), backlog item 4 · `crates/slink-format` reads, validates and scans; the JavaScript, Go and Rust implementations agree on 183 format cases and 31 scan cases with nine recorded differences; run on macOS arm64 only so far.

What the Rust side of `session/v0` is, how it is held to the validators that
already exist, and what comparing them found. The crate has no caller in the
product yet: `slink-rs build-check` links and runs it so that every target
proves it, and the first command to use it is the importer in backlog item 6.

## The crate

[crates/slink-format](../crates/slink-format) has four functions and no state.

| Function | What it does |
| --- | --- |
| `parse` | Reads a document's text as JSON, taking unpaired surrogate escapes as Go does |
| `validate` | Schema errors, at most 20, as `<JSON pointer> <message>`; on a document the schema accepts, the invariants |
| `invariant_errors` | Span ids are unique and no span is its own ancestor, in the words the JavaScript and Go validators use |
| `scan_for_secrets` | The credential scan: pattern name and masked preview, at most 10 findings |

The schema and the pattern list are embedded straight from
[packages/format](../packages/format), where they are authoritative. Go embeds
copies that `scripts/golden.mjs` keeps in step; Rust needs none.

## Choices

| Area | Choice | Reason |
| --- | --- | --- |
| Schema engine | [boon](https://github.com/santhosh-tekuri/boon) 0.6.1, formats asserted | The Rust library by the author of the one the Go CLI uses. Across the sweep below the two report the same number of errors at the same places. Its last release was 2025-01-07 |
| `date-time` | Our own check, [datetime.rs](../crates/slink-format/src/datetime.rs), registered over the library's | The three libraries disagree at the edges, and boon takes `+026-07-19T…` as a year. Ours is RFC 3339 section 5.6 and nothing more |
| Patterns | The `regex` crate with Unicode off | In Go's `regexp` and in a JavaScript `RegExp` without the `u` flag, `\b` is an ASCII boundary. With Unicode on, `\b` finds no boundary between `é` and `sk-`, and the key is missed |
| Previews | First eight bytes, never splitting a character, and the byte length | Every pattern matches ASCII only, so this is the text Go and JavaScript produce |
| Reading text | serde_json, after rewriting unpaired surrogate escapes to U+FFFD | See the first finding |
| Nesting | serde_json's limit: 128 levels are refused | See the fifth finding |
| Error wording | The library's own | JavaScript and Go already word errors differently. Where an error is reported is compared; what it says is not |

A pattern added to [secret-patterns.mjs](../packages/format/secret-patterns.mjs)
now has a second constraint beside staying RE2-compatible: it must compile
with Unicode off, which rules out `.` and negated classes. The crate's tests
fail on every target when one does not.

## Shared cases

[testdata/format](../testdata/format/README.md) holds the cases as data, and
each language's own tests read them: 45 documents that are valid, 107 that are
not, each with the place its error must be reported or, for a broken span
tree, the exact line, 22 texts that are not JSON, and nine whose verdict
differs by implementation. The 31 scan cases give the exact
findings. Before this, the Go validator was held to the JavaScript one by the
import goldens and four invalid documents written into a Go test.

The Rust test also holds every fixture that is a whole document to being valid
and free of anything the scan takes for a credential: the 33 import goldens,
the two format examples, and the viewer and share fixtures.

## Differential sweep

[format-diff.mjs](../scripts/format-diff.mjs) looks for differences no case
names. It walks every fixture document and changes one node at a time
(removed, replaced by each JSON type, lengthened, given an unknown field), then
adds lists aimed at the rules with an edge: 103 date-times, span ids, lengths
in three character widths, numbers as written, text that is barely JSON, and
43 strings shaped like credentials, each next to 21 different neighbours.
Every text goes to the JavaScript validator, the viewer's own, Go and Rust.

On 2026-10-12 it ran 5,871 texts, in under three seconds once the two helpers
are built. Verdicts, invariant errors, the places of schema errors and scan
findings agreed on all but 70, and each of those is one of the three kinds of
recorded difference below. A difference it cannot attribute to one of them
fails the run.

## What the comparison found

1. **Unpaired surrogate escapes.** A JavaScript string cut in the middle of an
   emoji is written out as a lone `\ud83d`. `JSON.parse` and Go read it;
   serde_json refuses the whole text. A transcript from any JavaScript agent
   can hold one, so `parse` reads each as U+FFFD, which is what Go does.
2. **Three readings of `date-time`.** ajv-formats takes a space or a tab for
   the `T` and an offset written `+0200` or `+02`. The Go library takes a sign
   where a digit belongs, as in `T+1:00:00Z`, and boon does that in the date
   too. Rust takes a time only when JavaScript and Go both do.
3. **Word boundaries.** The patterns begin and end with `\b`, which is ASCII
   in Go and JavaScript and Unicode by default in Rust. Unicode is off, and
   three scan cases fail if it is turned on.
4. **Numbers too large for a double.** JavaScript reads `1E400` as Infinity,
   which then passes any field the schema does not type and is written back as
   `null`. Go refuses the text, and Rust does the same.
5. **Nesting.** serde_json stops at 128 levels, Go at 10,000, and `JSON.parse`
   at whatever the stack allows. The limit stays for now, because parsing,
   dropping and walking a value all recurse and a stack overflow cannot be
   caught. The deepest fixture document has 11 levels.
6. **Go's test cache.** `go test` does not notice a change to a file outside
   the Go module, so an edited fixture under `testdata/` can show a cached
   pass. The fixture README says to use `-count=1`.

## Recorded differences

In every one Rust is on the side that refuses. On everything compared here, a
document Rust calls valid is valid to the validator the hosted service runs
and to the viewer's.

| Text | JavaScript | Go | Rust |
| --- | --- | --- | --- |
| A number too large for a double in a free-form field | valid | not JSON | not JSON |
| The same where the schema wants a count | invalid | not JSON | not JSON |
| A time with a space or a tab for the `T` | valid | invalid | invalid |
| An offset written `+0200` or `+02` | valid | invalid | invalid |
| A sign where a digit belongs in the time or offset | invalid | valid | invalid |
| 128 or more levels of nesting | valid | valid | not JSON |

The JavaScript and Go validators are unchanged. Tightening ajv's `date-time`
would change what the published `@session-link/format` accepts, which is a
format decision and not part of this slice.

## Cost

Measured on macOS arm64 with the release profile, through `slink-rs`.

| Build | Binary |
| --- | --- |
| Before this slice | 5.2 MiB |
| With the reader, the scan and `regex` | 5.9 MiB |
| With boon as well | 6.7 MiB |

The lockfile grew from 192 packages to 212. boon turns the `regex` crate's
Unicode tables on for the whole build, which is part of its 0.8 MiB. A
validator written for this one schema would save most of that and would be
ours to keep correct; the sweep is what would make that safe to try.

## Not done

- A run on Linux or Windows. `rust.yml` runs the crate's tests on all six
  targets and has a job for the sweep; neither has run yet.
- Reading a file. Decoding bytes that are not UTF-8 belongs with the first
  command that opens a document.
- Writing documents. Key order and how a number is written back matter to
  the importers and the exporter, and are settled there.
- Nesting past 127 levels, if a real transcript needs it when the importers
  and the recorder read provider payloads.
- `run/v0` is still accepted, because the schema still lists it.
- The plan's dependency review, which now has boon's release history to weigh.
