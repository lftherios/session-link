# Distributing the `slink` CLI

Two tiers, meeting users where they are:

1. **npm (`npx session.link`)** — the frictionless path for everyone with Node.
   The publishable package is `packages/cli/` (name `session.link`, bin `slink`,
   zero runtime deps). Its `prepack` bundles the whole CLI — ajv, the session/v0
   schema, and the viewer (as a string) all inlined — into `dist/slink.mjs`.
2. **Standalone binary** — the Node-less path via Homebrew, a `curl | sh`
   installer, and GitHub Release assets. `bun build --compile` embeds the Bun
   runtime + the same inlined assets into one per-platform executable.

## Build locally

```bash
npm run build:cli       # → packages/cli/dist/slink.mjs (what npm ships)
npm run build:binary    # → packages/cli/binaries/slink-<os>-<arch> (all targets)
npm run build:binary -- --host   # just this machine's target
```

Verify the npm tarball before publishing:

```bash
cd packages/cli && npm pack --dry-run   # expect 4 files, no app/next
```

## Releasing

`.github/workflows/release.yml` fires on a `v*` tag and does both:
`npm publish` from `packages/cli` (needs the `NPM_TOKEN` secret) and a
`bun`-cross-compiled binary matrix uploaded to a GitHub Release with
`SHA256SUMS.txt`.

```bash
git tag v0.1.0 && git push --tags
```

## One-time setup you still need to do

- **npm**: `npm login`, then add an `NPM_TOKEN` repo secret for CI. Claim the
  name `session.link` (available as of this writing) — and ideally the
  `@session-link` scope too, to reserve it.
- **Public releases**: the `curl | sh` and Homebrew channels download from
  GitHub Releases, which must be **public**. If the code repo stays private,
  create a separate public repo (or make releases public) and point
  `REPO`/`BASE` in `public/install.sh` and `packaging/homebrew-tap/Formula/slink.rb`
  at it. (npm is independent of this — it works with a private code repo.)
- **Homebrew tap**: create a public `<owner>/homebrew-tap` repo and put
  `Formula/slink.rb` there (the copy here is the template). Then
  `brew install <owner>/tap/slink` works. After each release, fill the four
  `sha256` values and the version in one step from the release's
  `SHA256SUMS.txt`:

  ```bash
  node scripts/update-formula.mjs v0.1.0 SHA256SUMS.txt path/to/Formula/slink.rb
  ```

  (Idempotent — safe to re-run. Point `BASE` in the formula at your public
  releases repo first.)
- **Installer**: `public/install.sh` is served at `https://session.link/install.sh`
  (it's a static file in `public/`). Docs headline: `curl -fsSL https://session.link/install.sh | sh`.

## P4: the Go distribution (built, not yet armed)

The Go CLI (`go/`) ships through four channels:

- **goreleaser** (`go/.goreleaser.yaml`) — compressed archives (~5MB vs
  the Bun binaries' 61MB), a checksums file, deb/rpm/apk, and Homebrew
  **cask** auto-update to `lftherios/homebrew-tap`.
- **`curl | sh`** — `packaging/install.sh`, archive-aware and
  checksum-verified, no Node.
- **npm** (`scripts/build-npm.mjs`) — the binary stays on npm via the
  platform-package + `optionalDependencies` pattern: per-platform
  `@session-link/cli-<goos>-<goarch>` packages (os/cpu-gated, binary
  only) + a `session.link` launcher whose `bin` shim resolves and execs
  the matching binary. Keeps `npx session.link` working; npm downloads
  only the ~5MB package matching the user's platform. The launcher
  **replaces** the JS `session.link` package at cutover.

**This is staged, not live.** Two gates keep a tag from shipping Go
binaries by accident, and neither the npm deprecation nor the cutover
happens until it's deliberately triggered:

1. `.github/workflows/release-go.yml` runs only when the repo variable
   `GO_RELEASE_ENABLED` is set to `on`.
2. goreleaser creates the GitHub Release as a **draft**.

### When cutover is authorized

1. Create a PAT with `contents:write` on `lftherios/homebrew-tap`; set it
   as the `HOMEBREW_TAP_TOKEN` repo secret.
2. Set repo variable `GO_RELEASE_ENABLED=on`.
3. Bump `go/cmd/slink/main.go`'s fallback `version` is unnecessary —
   goreleaser stamps `main.version` from the tag. Just tag: `git tag
   v0.3.0 && git push --tags`.
4. Review the draft release, then publish it.
5. Move `packaging/install.sh` to the server repo's `public/install.sh`
   (served at `https://session.link/install.sh`); update the README
   install section.
6. **npm stays as a binary channel** (not sunset): the release-go
   workflow's "publish npm binary channel" step runs
   `scripts/build-npm.mjs <version> --publish`, which publishes the six
   `@session-link/cli-*` platform packages and the `session.link`
   launcher. **At the same time, remove `session.link` from the JS
   `release.yml` publish loop** so the JS bundle and the Go launcher
   don't fight over the name. `@session-link/format` and
   `@session-link/viewer` keep publishing from `release.yml`.

Validate the config any time without releasing:
`cd go && goreleaser check` and
`goreleaser release --snapshot --clean --skip=publish` (writes `go/dist/`).
