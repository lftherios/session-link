# Working in this repository

This is the open client and format for session.link: the Go `slink` CLI and
always-on tap (`go/`), the `session/v0` format (`packages/format`), the React
viewer the CLI embeds (`packages/viewer`), and the pi extension
(`packages/pi-extension`). The hosted service is a separate repository,
`session-link-server`, and is not in this checkout. Anything that needs it
(encrypted or named publishing, account login) can only be exercised against
that repository's smoke server.

A Rust port of the CLI is under way in `crates/`, following
`docs/rust-migration.md`. Its binary, `slink-rs`, is not the product yet, and
Go remains the reference until the plan's P5. It knows every command, imports
Claude Code sessions, lists captures and serves the local viewer, and refuses
to publish; `docs/rust-cli.md` has what is ported. Nobody uses the software
yet, so the port migrates nothing and need not read state Go wrote.

## Before starting

- Check `git status` and the current branch. Work on the feature branch you
  were given; never commit directly to `main`.
- Read the docs index below and open the documents relevant to the task before
  reading code. Product direction is in `docs/product-plan.md`,
  `docs/success-criteria.md` and `docs/kpis.md`; every other doc describes a
  contract, a design or an implemented slice and carries its date and status
  under the title.

## Building and testing

The repo is a Node workspace, a Go module and a Cargo workspace. Install with
`npm ci`. CI uses Node 22 and Go 1.26; `rust-toolchain.toml` pins Rust.

Both native binaries embed generated files, and the viewer bundle and brand
files among them are gitignored, so build order matters in a fresh checkout:

1. `npm run build:viewer` writes the viewer artifact to `build/viewer/`: the
   bundle, `mark.svg`, `favicon.svg` and `brand.css` from `assets/brand/`, and
   a manifest of their digests. The Rust build reads that directory; the same
   files are copied to `go/internal/open/viewer.js` and
   `go/internal/open/branding/` for the Go embed. It also regenerates the
   tracked `packages/viewer/validate-session.js` from the schema. Run it before
   any `go build`, `go test` or `cargo` command, and again after viewer, brand
   or schema changes.
2. `node scripts/golden.mjs --check` verifies that the generated copies of the
   schema and secret patterns (`go/internal/format/session.schema.json`,
   `go/internal/scan/secret-patterns.json` and
   `packages/format/secret-patterns.json`) match their sources in
   `packages/format`. After changing the schema or
   `packages/format/secret-patterns.mjs`, run
   `node scripts/golden.mjs --update` and, for a schema change,
   `npm run build:viewer`; commit the regenerated files,
   `packages/viewer/validate-session.js` included.

| Task | Command |
| --- | --- |
| JS tests (format, viewer, pi extension) | `npm test` |
| Viewer typecheck | `npm run check:viewer` |
| Validate format examples | `npm run validate` |
| Go vet and tests | `cd go && go vet ./... && go test ./...` |
| Build the CLI | `cd go && go build -o slink ./cmd/slink` |
| Viewer preview with fixtures, no Go needed | `npm run dev:viewer` then open `http://127.0.0.1:4173` |
| npm binary channel smoke build | `node scripts/build-npm.mjs 0.0.0-ci` |
| Compatibility inventory matches the Go source | `node scripts/compat-inventory.mjs --check` |
| Differential runner against a built CLI | `SLINK_BINARY=go/slink node scripts/compat.mjs` |
| The same, with the Rust binary as the candidate | add `SLINK_CANDIDATE=target/release/slink-rs`; a name filter such as `help/` or `import/claude-code` runs the groups it has. `docs/rust-cli.md` has the count that agrees |
| JS, Go and Rust validators and scanners agree | `node scripts/format-diff.mjs`; it builds the Go and Rust helpers itself |
| Protocol case files are what their generator writes | `node scripts/protocol-cases.mjs --check` |
| The viewer, Go and Rust verifiers agree | `node scripts/protocol-diff.mjs`; add `--server ../session-link-server` to run the hosted verifier too |
| Rust format, lint and tests | `cargo fmt --all --check && cargo clippy --all-targets --locked -- -D warnings && cargo test --locked` |
| Build the Rust binary | `cargo build --release --locked`, which writes `target/release/slink-rs` |

CI (`.github/workflows/ci.yml`) has two jobs. `test` runs `node --check` over
`scripts/*.mjs` and `packages/format/*.mjs`, then `npm test`,
`npm run check:viewer`, `node scripts/golden.mjs --check` and
`node scripts/compat-inventory.mjs --check`. `go` runs
`npm run build:viewer`, the golden check, `go vet` and `go test`, the npm smoke
build and `npm run validate`. A second workflow, `rust.yml`, lints the Cargo
workspace, builds, tests and runs the release binary on a native runner for
each of the six targets, and runs the format and protocol cases and their
two sweeps, `format-diff.mjs` and `protocol-diff.mjs`, across the three
languages; it also runs on `feature/rust-**` branches.
`docs/rust-build.md` has the choices behind it. Run the parts your change
touches before finishing.

## Verifying in the real product

- Run the CLI with an isolated home: `SLINK_HOME=$(mktemp -d) go/slink view
  --session testdata/viewer/arrival/session.json --no-browser`. Never point
  checks at the real `~/.slink`; it holds the user's captures, previews, drafts
  and share receipts. The same holds for `slink-rs`: its tests in
  `crates/slink/tests/cli.rs` give it a home of its own, fail if a command
  that should not write leaves anything there, and check that the commands
  that do write leave nothing outside the slink home.
- `node scripts/check-security-browser.mjs` checks private capture permissions,
  local viewer access, framing restrictions and external-image privacy.
- `node scripts/check-viewer-browser.mjs` drives the built binary in a headless
  Chromium-family browser. It and the security check read `SLINK_BINARY`
  (default `/tmp/session-link-slink`) and `BROWSER_BINARY`. Given `slink-rs`
  it passes as far as reading goes and stops where composing a view begins.
- `node scripts/check-protocol-browser.mjs` runs the protocol cases through the
  viewer's code in a browser, whose Web Crypto is not Node's. It needs no
  binary and no server, and reads `BROWSER_BINARY`.
- The encrypted, identity and named checks in `scripts/` need the server repo's
  smoke server. Run them from the server checkout with
  `SLINK_BROWSER_SCRIPT=<path to the check> npm run smoke:encrypted`; the smoke
  passes `SLINK_BROWSER_BASE`, `SLINK_BROWSER_LINK` and `SLINK_BROWSER_MAIL`.
  These three take the browser from `CHROME_PATH`, not `BROWSER_BINARY`.
- Fixtures under `testdata/` are fictional. Never add a real transcript, and
  keep the leakage markers the fixture READMEs describe intact.
- The README opens with a recording of the landing page's "How it works"
  section, `assets/product/how-it-works.gif`. It is generated too:
  `node scripts/capture-how-it-works.mjs` plays the section from the server
  checkout's `public/landing.html` in a headless browser and writes the GIF
  with `scripts/gif.mjs`. Record it again when that section changes.
- Screenshots are generated, not edited: `node scripts/capture-product.mjs`,
  whose `SLINK_BINARY` defaults to `/tmp/session-link-landing-slink`
  (`assets/product/README.md` has the steps). The social preview PNG is
  exported from its SVG with `node scripts/capture-social.mjs`.

## Process hygiene

- Stop viewers you started by their PID, Ctrl-C in the foreground, or the
  **Stop viewer** button on the sessions page. Background viewers log to
  `$SLINK_HOME/viewer-*.log`.
- Never kill by a broad pattern such as `pkill -f slink`. The user's always-on
  recorder (`slink tap --install`) and unrelated viewers match it.

## Contracts to respect

- `session/v0` is pre-1.0 and open-world: unknown span kinds, roles, content
  parts and fields must validate and round-trip. The canonical schema is
  `packages/format/session.schema.json`; the Go copy is generated.
- Secret patterns have one source, `packages/format/secret-patterns.mjs`.
- `testdata/format` holds the cases the JavaScript, Go and Rust validators and
  scanners must all agree on. A change to a schema rule or a pattern adds its
  case there in the same change; the README there has the fields, and
  `docs/rust-format.md` the differences that are recorded on purpose. Go
  caches test results across fixture edits, so use `go test -count=1`.
- `testdata/protocol` holds the envelopes, signed statements, device histories
  and vault records that the viewer, Go and Rust verifiers must all judge
  alike. The files are generated, because a case needs signatures: a change to
  a rule in any of the three adds its case to `scripts/protocol-cases.mjs`,
  which is then run with `--update`. `docs/rust-protocol.md` has the
  differences that are recorded on purpose; in each of them Rust refuses.
- The `slink help` text in `go/cmd/slink/help.go` is the CLI spec. Keep its
  `commands` list in sync with the switch in `dispatch()` in `main.go`.
- `testdata/compat/cli.json` is the command, flag and environment-variable
  surface the Rust port is held to, generated from the Go source. After
  changing any of those, run `node scripts/compat-inventory.mjs --update` and
  commit the file; add `--server ../session-link-server` after a hosted route
  changes. `docs/compatibility-baseline.md` says what the differential runner,
  `scripts/compat.mjs`, covers.
- Fidelity is honest: `exact` for recorded traffic, `reconstructed` for
  imports. Missing data is shown as missing, never invented (success
  criterion 7).
- Nothing leaves the machine without the user's publish action, and new clients
  never fall back to plaintext upload.

## Documentation

- Docs point at files and explain decisions; they do not paste code. New docs
  follow the existing shape: title, a date and status line, then sections.
- `docs/index.md` is the index. When you add, remove or rename a doc anywhere
  in the repo, update its row there in the same change, keeping the summary
  under 150 characters.
- When behavior a doc describes changes, update that doc. The named-sharing
  doc carries an implementation checklist that must stay true.
- `docs/user-guide.md` is the user-facing guide and the source of the site's
  `/docs` page. When something a user sees changes (a command, a flag, a label,
  a limit), update it, then rebuild the page into the server checkout with
  `node scripts/build-docs.mjs ../session-link-server/public/docs.html`.
- `docs/guides/` holds one page for each question people search with: how to
  share a session from a named agent, and how `slink` compares with other
  tools. The same command builds each into `public/docs/<file name>.html`,
  served at `/docs/<file name>`. A page's title is its first heading and its
  search description is its opening sentence, so both stay short; the test in
  `test/user-guide.test.mjs` holds them to that. A new guide also needs a line
  in the server's `public/sitemap.xml` and `public/llms.txt`. The comparison
  states only what each tool's own documentation says, with the date it was
  read.

## Releases

The README and the landing page say the binary is under 15 MB installed, and
the README that it is about 6 MB to download. Both are true of v0.9.0 on every
platform (the largest binary is 14.7 MB, the largest archive 6.1 MB) and
nothing checks them. Before a release, look at the sizes the npm smoke build
prints or leaves in `go/dist-npm`; if one passes the figure, change the README,
the landing page's card and the recording together.

Tags `v*` drive releases. `release.yml` publishes `@session-link/format` and
`@session-link/viewer` when their `package.json` versions are not yet on npm.
On a tag push, `release-go.yml` runs goreleaser and the npm binary channel only
when the `GO_RELEASE_ENABLED` repository variable is `on`. A manual dispatch
with a `version` runs the npm job alone and is not gated. Channels are
described in `packaging/README.md`.

@docs/index.md
