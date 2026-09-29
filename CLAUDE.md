# Working in this repository

This is the open client and format for session.link: the Go `slink` CLI and
always-on tap (`go/`), the `session/v0` format (`packages/format`), the React
viewer the CLI embeds (`packages/viewer`), and the pi extension
(`packages/pi-extension`). The hosted service is a separate repository,
`session-link-server`, and is not in this checkout. Anything that needs it
(encrypted or named publishing, account login) can only be exercised against
that repository's smoke server.

## Before starting

- Check `git status` and the current branch. Work on the feature branch you
  were given; never commit directly to `main`.
- Read the docs index below and open the documents relevant to the task before
  reading code. Product direction is in `docs/product-plan.md`,
  `docs/success-criteria.md` and `docs/kpis.md`; every other doc describes a
  contract, a design or an implemented slice and carries its date and status
  under the title.

## Building and testing

The repo is a Node workspace plus a Go module. Install with `npm ci`.

The Go binary `go:embed`s generated files, and two of them are gitignored, so
build order matters in a fresh checkout:

1. `npm run build:viewer` writes `go/internal/open/viewer.js` and copies
   `assets/brand/` into `go/internal/open/branding/`. Run it before any
   `go build` or `go test`, and again after viewer or brand changes.
2. `node scripts/golden.mjs --check` verifies that the committed copies of the
   schema and secret patterns under `go/internal/` match `packages/format`.
   After changing the schema or `packages/format/secret-patterns.mjs`, run
   `node scripts/golden.mjs --update` and commit the regenerated files.

| Task | Command |
| --- | --- |
| JS tests (format, viewer, pi extension) | `npm test` |
| Viewer typecheck | `npm run check:viewer` |
| Validate format examples | `npm run validate` |
| Go vet and tests | `cd go && go vet ./... && go test ./...` |
| Build the CLI | `cd go && go build -o slink ./cmd/slink` |
| Viewer preview with fixtures, no Go needed | `npm run dev:viewer` then open `http://127.0.0.1:4173` |
| npm binary channel smoke build | `node scripts/build-npm.mjs 0.0.0-ci` |

CI (`.github/workflows/ci.yml`) runs `node --check` over `scripts/*.mjs` and
`packages/format/*.mjs`, then `npm test`, `npm run check:viewer`,
`node scripts/golden.mjs --check`, `go vet` and `go test`, the npm smoke build
and `npm run validate`. Run the parts your change touches before finishing.

## Verifying in the real product

- Run the CLI with an isolated home: `SLINK_HOME=$(mktemp -d) go/slink view
  --session testdata/viewer/arrival/session.json --no-browser`. Never point
  checks at the real `~/.slink`; it holds the user's captures, previews, drafts
  and share receipts.
- `node scripts/check-viewer-browser.mjs` drives the built binary in a headless
  Chromium-family browser. It reads `SLINK_BINARY` (default
  `/tmp/session-link-slink`) and `BROWSER_BINARY`. The encrypted, identity and
  named checks in `scripts/` need the server repo's smoke server and are
  invoked from there via `SLINK_BROWSER_BASE` and `SLINK_BROWSER_MAIL`.
- Fixtures under `testdata/` are fictional. Never add a real transcript, and
  keep the leakage markers the fixture READMEs describe intact.
- Screenshots and the demo GIF are generated, not edited:
  `node scripts/capture-product.mjs` and `vhs assets/demo.tape`.

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
- The `slink help` text in `go/cmd/slink/help.go` is the CLI spec. Keep its
  `commands` list in sync with the switch in `main()`.
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
- When behavior a doc describes changes, update that doc. The identity and
  named-sharing docs carry implementation checklists that must stay true.

## Releases

Tags `v*` drive releases. `release.yml` publishes `@session-link/format` and
`@session-link/viewer` when their `package.json` versions are not yet on npm.
`release-go.yml` runs goreleaser and the npm binary channel only when the
`GO_RELEASE_ENABLED` repository variable is `on`. Channels are described in
`packaging/README.md`.

@docs/index.md
