# Format and scan cases

The cases every `session/v0` validator and credential scanner is held to. The
documents are fictional and as small as the rule they test. Every value shaped
like a key is a prefix followed by one repeated character.

Three test suites read the two files and each must agree with every entry:
[format-cases.test.mjs](../../test/format-cases.test.mjs) for the JavaScript
validator and the viewer's own,
[cases_test.go](../../go/internal/format/cases_test.go) and
[its counterpart for the scan](../../go/internal/scan/cases_test.go) for Go, and
[cases.rs](../../crates/slink-format/tests/cases.rs) for Rust.
[rust-format.md](../../docs/rust-format.md) has the decisions behind the
entries that differ.

## cases.json

| Field | Meaning |
| --- | --- |
| `name` | What the case is, unique in the file |
| `doc` | The document, when any reader reads it the same way |
| `text` | The document's text, when how it is written is the point: a number's spelling, an escape, something that is not JSON |
| `expect` | `valid`, `invalid` or `unreadable` (the text is not JSON), or an object with one verdict each for `js`, `go` and `rust` |
| `at` | For a schema failure, a JSON pointer at which an error must be reported; `/` is the document |
| `error` | For a broken tree, the one error line, identical in every implementation |
| `count` | How many error lines there must be, for the limit of 20 |
| `why` | For a case whose verdicts differ, the reason |

A case with one verdict per implementation records a difference that is known
and decided. Add one only with its `why`, and describe the decision in
[rust-format.md](../../docs/rust-format.md).

## scan.json

Each entry has a `name`, the `text` to scan and the `hits` it must produce, in
order: the pattern's name and the masked preview.

## Changing them

The files are ASCII: a character outside it is written as a `\u` escape, so an
invisible one cannot hide in a fixture. Go caches a test result without
noticing a change to a file outside its module, so run its tests with
`-count=1` after editing a case. A new rule in the schema or a new pattern
gets a case here in the same change, and
[format-diff.mjs](../../scripts/format-diff.mjs) is the way to look for
differences no case names yet.
