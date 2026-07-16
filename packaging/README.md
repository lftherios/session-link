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
