# Migrating the daemon and CLI to Go

Decision (2026-07-18): the `slink` daemon (tap) and CLI move to Go. The
motivations are measured, not aesthetic: the Bun-compiled binaries are
61–95MB where Go lands ~10–15MB, and the always-on tap idles at ~70MB RSS
on Node where a Go daemon idles at ~10–20MB. Everything else about the
product stays where it is.

**What does NOT move:** the hosted server (Next.js), `@session-link/viewer`
(React — it *is* the product's rendering, embedded by the Go CLI as a
static bundle), and `@session-link/format` as the reference implementation
and schema home. The JS CLI remains in-tree as the reference
implementation and parity oracle until the cutover completes.

## Contracts (frozen inputs to the Go implementation)

The Go code implements contracts, not the JS code's shape:

1. **`session/v0`** — the JSON Schema in `packages/format/schema/`.
   Acceptance is byte-level: same inputs → equivalent documents (timestamps
   normalized) vs the JS implementation.
2. **The spool protocol** (as of commit `8a74941`, pending round-3
   verification): `<capture>.json.spool` JSONL — line 1 is the run
   skeleton (must carry `.schema`; assembly refuses otherwise), one span
   per line, U+2028/U+2029 escaped, appends begin on a fresh line;
   `.spool.pid` owner sidecar `{pid, boot}` with EPERM-is-alive and
   boot-token semantics; `<capture>.lock` O_EXCL commit mutex with age
   break; snapshot-vs-finalize rules; `.corrupt` set-aside. A Go tap and a
   JS CLI (or vice versa) must interoperate on the same `~/.slink` during
   the transition.
3. **Secret patterns** — exported from `packages/format` as a plain JSON
   artifact consumed by both the Go client and the TS server, preserving
   the single-source guarantee.
4. **The CLI surface** — the `slink help` text is the spec; flags and
   behaviors carry over exactly.
5. **The server API** — ingest/login endpoints as exercised by the JS e2e
   tests.

## Phases

- **P0 — contract extraction.** Secret patterns → JSON artifact; golden
  fixtures: recorded proxy streams (Anthropic + OpenAI SSE, chat +
  responses), importer inputs, and their JS-produced `session/v0` outputs,
  checked into `testdata/`. CI job that runs both implementations over the
  fixtures and diffs.
- **P1 — the daemon.** `go/` workspace at the repo root
  (`module github.com/lftherios/session-link`), `cmd/slink`. Tap only:
  proxy, streaming tee, SSE reassembly, spool append, segmentation,
  pid/lock/recovery. This is the always-resident code — the footprint win
  — and the hardest 20% (SSE reassembly fidelity). Opt-in:
  `slink tap --go` or a separate binary name until parity holds.
- **P2 — CLI core.** `push` (validate via schema + secret scan), `login`,
  `list`, `share`, `prune`, `open` (viewer bundle via `go:embed`),
  `on`/`off`, service install.
- **P3 — importers.** claude-code/codex/pi (file-based) first;
  opencode/hermes via `modernc.org/sqlite` (pure Go, no cgo). This also
  removes the Node ≥22.13 constraint for SQLite importers.
- **P4 — distribution cutover.** goreleaser: tar.gz/zip archives +
  checksums, GitHub Release, Homebrew tap auto-update (retires
  `update-formula.mjs` + manual copy), deb/rpm. npm keeps working via
  platform packages (`optionalDependencies` + tiny JS launcher, the
  esbuild pattern, ~4MB per platform). `install.sh` switches to archives.
  First Go release = v0.3.0. JS CLI enters maintenance as the reference
  implementation.

## Open decisions (recommendations marked)

- **npm shape**: keep npm with platform binaries *(recommended)* vs drop
  npm. Dropping abandons the `npx session.link` funnel; keeping costs the
  shim package complexity.
- **Importer phasing**: P3 as listed *(recommended)* vs hybrid-forever
  (importers stay JS, Go shells out) — hybrid keeps the community
  contribution bar low but reintroduces the Node dependency the migration
  removes.
- **Freeze `session/v0` during P1–P3** *(recommended)* vs dual-implement
  format changes as they land.

## Risks

- **SSE reassembly divergence** — the highest-fidelity-risk port
  (`normalize.mjs`); mitigated by golden streams in P0 and byte-diff CI.
- **Velocity dip** — the JS repo shipped ~20 features in 3 days; Go will
  not. Accepted in the decision.
- **Interop window** — a Go tap and JS CLI sharing `~/.slink` mid-
  transition; mitigated by the spool protocol being the tested contract.
- **Format churn** — see the freeze recommendation.
