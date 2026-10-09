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
(cd go && goreleaser check)                            # validate the release config
(cd go && goreleaser release --snapshot --clean --skip=publish)   # full build → go/dist/
node scripts/build-npm.mjs 0.0.0-ci                    # cross-compile the npm channel (no publish)
```

## Releasing (CI)

`.github/workflows/release-go.yml` fires on a `v*` tag and runs goreleaser
plus the npm binary-channel publish. A tag push is **gated**: it runs only
when the repo variable `GO_RELEASE_ENABLED == 'on'`. A manual dispatch with a
`version` runs the npm job alone for an existing tag and is not gated.
goreleaser publishes the release at once, not as a draft, because the Homebrew
cask points at its assets.

The variable is `on` in this repository (checked 2026-10-09). Arming a
repository is one-time:

1. Name the release workflows as trusted publishers of the npm packages
   (below). No npm token is stored.
2. Mint a GitHub fine-grained PAT with `contents:write` on
   `lftherios/homebrew-tap` → `gh secret set HOMEBREW_TAP_TOKEN`.
3. `gh variable set GO_RELEASE_ENABLED --body on` — **last**, or a tag pushed
   before the rest is in place fails.

Then a release is just `git tag vX.Y.Z && git push --tags`. `release.yml`
(separate) publishes only `@session-link/format` + `@session-link/viewer`;
the Go launcher owns `session.link`.

### npm trusted publishing

npm accepts a publish from these workflows because each package names the
workflow file as a trusted publisher, and GitHub proves to npm which workflow
is running. Nothing is stored in the repository's secrets, and a published
version carries a provenance record. It needs npm 11.5.1 or later in the
workflow, which is why both install npm 11 before publishing.

A package owner sets it once per package, signed in to npm with two-factor
authentication on, using npm 11.15 or later:

```bash
npm login
for p in session.link @session-link/cli-{darwin,linux,windows}-{amd64,arm64}; do
  npx -y npm@latest trust github "$p" --file release-go.yml --repo lftherios/session-link --allow-publish --yes
done
for p in @session-link/format @session-link/viewer; do
  npx -y npm@latest trust github "$p" --file release.yml --repo lftherios/session-link --allow-publish --yes
done
npx -y npm@latest trust list session.link    # check one
```

The same setting is on npmjs.com under each package's Settings → Trusted
Publisher. If a publish fails, fix the cause and repeat it without a new tag: `gh workflow run release-go.yml -f
version=<x.y.z>` for the binary channel, `gh workflow run release.yml` for the
libraries. Both skip versions already on npm.

The v0.3.0 cutover was performed locally (goreleaser with a `gh auth token`,
npm published by hand) before CI was armed.
