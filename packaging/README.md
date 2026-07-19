# Distributing the `slink` CLI

As of v0.3.0 the CLI is a single native Go binary (`go/`). It ships through
four channels, all driven from the tag:

- **goreleaser** (`go/.goreleaser.yaml`) — compressed archives (~5MB), a
  `checksums.txt`, `.deb`/`.rpm`/`.apk`, and Homebrew **cask** auto-update
  to `lftherios/homebrew-tap`.
- **`curl | sh`** — `packaging/install.sh` (archive-aware, checksum-verified,
  no Node), served at `https://session.link/install.sh` from the server
  repo's `public/`.
- **npm** (`scripts/build-npm.mjs`) — the binary on npm via the
  platform-package + `optionalDependencies` pattern (esbuild/Biome style):
  per-platform `@session-link/cli-<goos>-<goarch>` packages (os/cpu-gated,
  binary only) + a `session.link` launcher whose `bin` shim resolves and
  execs the matching binary. `npx session.link` fetches only the ~5MB
  package for the user's platform.
- **GitHub Release** — every archive + package + checksum, attached by
  goreleaser.

The pre-Go Node/Bun distribution (`packages/cli`, `build-cli.mjs`,
`build-binary.mjs`, `update-formula.mjs`) was removed at cutover; its last
state is tagged `js-cli-v0.2`.

## Build / validate locally (no release)

```bash
npm run build:viewer                                   # → go/internal/open/viewer.js (go:embed'd)
node scripts/golden.mjs --check                        # Go embeds in sync with packages/format
cd go && goreleaser check                              # validate the release config
cd go && goreleaser release --snapshot --clean --skip=publish   # full build → go/dist/
node scripts/build-npm.mjs 0.0.0-ci                    # cross-compile the npm channel (no publish)
```

## Releasing (CI, once armed)

`.github/workflows/release-go.yml` fires on a `v*` tag and runs goreleaser
plus the npm binary-channel publish. It is **gated**: it runs only when the
repo variable `GO_RELEASE_ENABLED == 'on'`, and goreleaser creates the
release as a draft.

To arm it (one-time):

1. Mint an npm **automation** token → `gh secret set NPM_TOKEN` (bypasses
   the per-publish OTP).
2. Mint a GitHub fine-grained PAT with `contents:write` on
   `lftherios/homebrew-tap` → `gh secret set HOMEBREW_TAP_TOKEN`.
3. `gh variable set GO_RELEASE_ENABLED --body on` — **last**, or a tag pushed
   before the secrets exist fails.

Then a release is just `git tag v0.3.1 && git push --tags` → review the draft
→ publish. `release.yml` (separate) publishes only `@session-link/format` +
`@session-link/viewer`; the Go launcher owns `session.link`.

The v0.3.0 cutover was performed locally (goreleaser with a `gh auth token`,
npm published by hand) before CI was armed — see the go-migration memory for
the exact steps.
