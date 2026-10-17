# Rust workspace and build

2026-10-12, updated 2026-10-16 · Implemented slice of the [Rust migration](rust-migration.md)'s P1 · the workspace and the viewer artifact, built, tested and run natively on all six targets in CI.

How the Rust side of the repository is built, which choices the first build
fixed, and what it measured. The binary it produces, `slink-rs`, is not the
product yet. When this slice was written it had only `version` and
`build-check`, a check of its own build, and existed to prove the toolchain,
the native dependencies and the viewer embed on every distribution target
before the port depended on them. It now knows every command and runs all
but the recorder's; [rust-cli.md](rust-cli.md) and
[rust-client.md](rust-client.md) have what they do.

## Layout

| File | What it holds |
| --- | --- |
| [Cargo.toml](../Cargo.toml) | The workspace: every directory under `crates/`, a lint that forbids unsafe code, and the release profile. The CLI crate denies it instead, for one call that [rust-cli.md](rust-cli.md#what-building-it-found) explains |
| [rust-toolchain.toml](../rust-toolchain.toml) | The pinned toolchain, 1.98.0, with rustfmt and clippy |
| [.cargo/config.toml](../.cargo/config.toml) | Static C runtime on Windows and the macOS deployment target |
| [Cargo.lock](../Cargo.lock) | Committed; every command in CI passes `--locked` |
| [crates/slink](../crates/slink) | The CLI crate. Its binary is named `slink-rs` until it replaces Go |
| [crates/slink-format](../crates/slink-format) | Reading, validating and scanning `session/v0`; [rust-format.md](rust-format.md) describes it |
| [crates/slink-protocol](../crates/slink-protocol) | Opening and making envelopes, signed statements, keys and wrappers; verifying device histories and vault records. [rust-protocol.md](rust-protocol.md) describes it |
| [crates/slink-local](../crates/slink-local), [crates/slink-import](../crates/slink-import) | The slink home and saved previews; the importers and session discovery. [rust-cli.md](rust-cli.md) describes both and the commands built on them |
| [crates/slink-client](../crates/slink-client) | The hosted service as the CLI reaches it: sign-in, publishing, deleting, device keys, the vault and named shares. [rust-client.md](rust-client.md) describes it |
| [rust.yml](../.github/workflows/rust.yml) | Lint, then build, test and run on six native runners; compare the format validators and the protocol verifiers across languages |

Crates are added when their first code lands, as the plan asks.

## Viewer artifact

[build-viewer.mjs](../scripts/build-viewer.mjs) now writes the bundle, the
three brand files and `manifest.json` to `build/viewer/`, which is gitignored.
The manifest records each file's size and SHA-256, the viewer and format
package versions, and the commit, with `dirty` set when anything the bundle is
built from differs from that commit.

- The Go embed under `go/internal/open/` is copied from that directory, so the
  two implementations cannot embed different bundles. Moving the build did
  not change its bytes: at that commit the bundle's digest was still the one the
  [baseline](compatibility-baseline.md) pins.
- The Rust build reads the directory in place.
  [build.rs](../crates/slink/build.rs) checks every file against the manifest
  and embeds what the manifest lists, so a file added to the artifact needs no
  Rust change. `SLINK_VIEWER_DIR` points it at another directory. With no
  artifact the build stops and says to run `npm run build:viewer`.

This is most of P1's viewer-artifact item. Two parts are open: the manifest
has no protocol versions, because the viewer has no single place that states
them, and the server's copy of the bundle is still written by
`--hosted-output`.

## Choices the build fixed

| Area | Choice | Reason |
| --- | --- | --- |
| HTTP server | axum on Tokio, HTTP/1 only, default features off | The plan's default; the loopback server needs nothing more yet |
| HTTP client | reqwest with rustls | No OpenSSL to find or ship on any target |
| Trust store | The operating system's, through `rustls-platform-verifier` | What Go's client used; a machine behind an inspecting proxy keeps working |
| Crypto provider | ring, installed in [transport.rs](../crates/slink-client/src/transport.rs) | Needs only a C compiler, and clang on Windows ARM64, which the runner has. reqwest's default, aws-lc-rs, was not tried |
| SQLite | rusqlite with the bundled library, 3.53.2 | Readers behave the same on every machine; costs a C compiler at build time, which pure-Go builds never needed |
| Local time | chrono, with only its clock | Aider writes local times with no zone and the sessions page groups by local day; finding the system's zone is different work on each platform |
| Zstandard | ruzstd, a decoder in Rust | DeepSeek Harness compresses its logs. Reading is all that is needed, and the C library would be a second thing to compile on six targets |
| Linux | musl targets, fully static | One binary for glibc distributions and Alpine, as the `.apk` package needs |
| Windows | MSVC targets with the C runtime linked in | No redistributable to install |
| macOS | Deployment target 12.0 on both architectures | A floor had to be stated; confirm it is the one wanted |
| Panics | Unwind, not abort | A panic while handling one proxied request must not stop the recorder |

The lockfile held 192 packages across all targets at this slice, development
dependencies included, 212 with the format crate, 213 with the protocol
crate, which brought no package but itself, 217 with the first commands, and
228 with all eight importers: chrono and ruzstd and what they bring.
The plan's
dependency review has not been done.

## Targets

Each target is built on a runner of its own architecture, so tests and the
release binary run natively and nothing depends on a cross toolchain.

| Target | Runner | Release binary |
| --- | --- | --- |
| `aarch64-apple-darwin` | `macos-15` | 5.2 MiB |
| `x86_64-apple-darwin` | `macos-15-intel` | 5.4 MiB |
| `x86_64-unknown-linux-musl` | `ubuntu-24.04` | 6.1 MiB |
| `aarch64-unknown-linux-musl` | `ubuntu-24.04-arm` | 5.5 MiB |
| `x86_64-pc-windows-msvc` | `windows-2025` | 6.2 MiB |
| `aarch64-pc-windows-msvc` | `windows-11-arm` | 5.5 MiB |

All six passed on the workflow's first run, on 2026-10-12 at commit `f4298dd`:
the tests, the trust-store test against the hosted service, and `build-check`
from the release binary, each on its own architecture. The run took five and a
half minutes with empty caches, the Intel macOS job being the slowest.

The workflow triggers on `main` and on `feature/rust-**` branches, because this
repository merges without pull requests and the existing CI would not see the
branch. Beyond the Rust toolchain, the runners needed only `musl-tools` on
Linux. Each build job stops after thirty minutes; the jobs take about four,
and a test that waited for ever once held two runners for hours.

What this does not show is installation: the binaries ran where they were
built, on machines with a development toolchain. Running them on a clean
machine belongs with packaging in P5. The sizes are those of that commit; the
format crate has since added 1.5 to 1.9 MiB to each, and
[rust-format.md](rust-format.md#cost) has the table.

## What is checked

- `slink-rs version` prints what the Go binary prints. With `slink-rs` as the
  candidate, the [differential runner](../scripts/compat.mjs) agrees on the
  version cases.
- `slink-rs build-check` runs what the binary links: a round trip through the
  bundled SQLite, the embedded files against their manifest digests, the
  bundle served by axum on loopback and fetched by the client every request to
  the service uses (`slink_client::client`), the
  format crate's schema and credential patterns compiled and applied, and the
  protocol crate opening two envelopes, judging the shared signed statements
  and making and opening a wrapper. The commands now reach the same code, and
  it stays for CI: the tests run a test build, and `build-check` is the one
  step in [rust.yml](../.github/workflows/rust.yml) that runs the release
  binary, with its LTO and stripped symbols, on each target's own runner. It
  needs no home, network or terminal. It is not a product command, so it is
  in neither the help nor the inventory.
- [tests/cli.rs](../crates/slink/tests/cli.rs) runs the built binary with an
  empty environment and a home of its own, and fails if the home is not empty
  afterwards. That is the plan's rule that no Rust executable writes to a real
  home, held as a test from the first command.
- [tests/tls.rs](../crates/slink/tests/tls.rs) has a reqwest client told to
  trust a test certificate complete a handshake, and the shipped client refuse
  that certificate. A third test, ignored by default because it reaches the
  network, has the shipped client verify `https://session.link/api/health`
  through the system trust store; CI runs it on every target.

## First measurements

On an Apple M1 Max with macOS 26.6.2, release profile.

| Measurement | Go `slink` | `slink-rs` |
| --- | --- | --- |
| Binary, darwin/arm64 | 13.2 MiB | 5.2 MiB |
| `version`, median of 20 runs including process start, quietest run | 18.1 ms | 6.5 ms |

The machine was busy with unrelated work during the timing, so the absolute
figures are not a baseline: over six runs of the same loop Go's median ranged
from 18 to 41 ms and `slink-rs` from 6.5 to 16 ms. Within each run `slink-rs`
started 2.0 to 2.8 times faster. The baseline's 13.7 ms for Go came from a
quieter day.

The Rust figures were a floor, not a forecast: at that commit the binary held
SQLite, TLS, the HTTP client and server, and the viewer, and none of the
importers, the recorder, the envelope code or the CLI itself. With everything
but the recorder it is 9,033,360 bytes on macOS arm64
([rust-client.md](rust-client.md#cost)).

## Not done

- Packaging. Archives, checksums, the npm platform packages, Homebrew and the
  Linux packages still come from the Go build; replacing them is P5.
- The differential runner in CI, which waits for a Linux run of it.
- Measuring musl's allocator under the recorder's load, when there is a
  recorder to measure.
