# Documentation index

One row per document, each with a summary under 150 characters. `CLAUDE.md`
includes this file with `@docs/index.md`, so it is always in agent context.
When a doc is added, removed or renamed, update its row here in the same change.
Documents in `docs/` are dated working documents; the date line under each
title says when it was written and whether it describes a decision, a design
or an implemented slice.

## For users

| File | Summary |
|------|---------|
| [user-guide.md](user-guide.md) | User guide for `slink` 0.9.1: install, read, share, record, supported agents, privacy, scripts, commands, files, troubleshooting |
| [guides/claude-code.md](guides/claude-code.md) | Site page `/docs/claude-code`: reading and sharing a Claude Code session, what its transcript holds, what `/export` does by itself |
| [guides/codex.md](guides/codex.md) | Site page `/docs/codex`: reading and sharing a Codex CLI session, what a rollout holds, why `slink record` captures nothing from Codex |
| [guides/opencode.md](guides/opencode.md) | Site page `/docs/opencode`: reading and sharing an opencode session, what its database holds, how this differs from opencode's `/share` |
| [guides/aider.md](guides/aider.md) | Site page `/docs/aider`: reading and sharing a run from `.aider.chat.history.md`, and what Aider's history leaves out |
| [guides/pi.md](guides/pi.md) | Site page `/docs/pi`: sharing a pi session with the extension's `/slink` commands or from the terminal; exact and reconstructed capture |
| [guides/compare.md](guides/compare.md) | Site page `/docs/compare`: `slink` beside four other session-sharing tools and the agents' own commands, from their docs on 2026-10-09 |

## Product direction

| File | Summary |
|------|---------|
| [product-plan.md](product-plan.md) | Product promise and thesis, the three handoff use cases, six milestones in dependency order, and the integration TODO; draft updated 2026-10-12 |
| [success-criteria.md](success-criteria.md) | The nine agreed requirements: install, daemon, integrations, viewer, data control, sharing, trustworthy record, no disruption, recipient value |
| [kpis.md](kpis.md) | Proposed measurable targets and measurement definitions per criterion plus a week-two retention goal; proposed targets, not measured baselines |
| [project-review.md](project-review.md) | Review of the 2026-09-11 checkout: CLI, format, viewer and test state, fixes made during the review, and the next viewer priorities |

## System architecture

| File | Summary |
|------|---------|
| [architecture-overview.md](architecture-overview.md) | One-page map of the CLI, shared viewer, hosted service, encrypted sharing, trust boundaries and current deployment |
| [architecture-review.md](architecture-review.md) | Whole-system review with code evidence, nine architectural findings, live topology and verification results |
| [architecture-alternatives.md](architecture-alternatives.md) | Two alternatives: a transactional monolith and a stateless hosted platform, with trade-offs, migration paths and a recommendation |
| [rust-migration.md](rust-migration.md) | Agreed Rust direction: replace Go CLI and hosted runtime, share protocol code, migrate nothing while there are no users, milestones with exit gates |
| [compatibility-baseline.md](compatibility-baseline.md) | P0 reference for the Rust port: pinned revisions, CLI and hosted inventories, the differential runner, what it covers, findings and gaps |
| [rust-build.md](rust-build.md) | The Rust workspace as built: pinned toolchain, viewer artifact, TLS and SQLite choices, the six targets and their state, first measurements |
| [rust-format.md](rust-format.md) | `crates/slink-format`: reading, validating and scanning `session/v0`, the shared cases, the sweep across JS, Go and Rust, recorded differences |
| [rust-cli.md](rust-cli.md) | `slink-rs` so far: eight importers, the local viewer and sessions page, drafts and excerpt export; what agrees with Go and what was found in it |
| [rust-client.md](rust-client.md) | `crates/slink-client`: sign-in, encrypted links, a lost upload resumed, delete, device keys and the vault; where Rust refuses what Go accepts |
| [rust-protocol.md](rust-protocol.md) | `crates/slink-protocol`: envelopes, histories and vaults verified; keys, wrappers, requests and named shares made; shared cases, sweep, findings |

## Viewer and sharing experience

| File | Summary |
|------|---------|
| [viewer-model.md](viewer-model.md) | Agreed viewer content model: session metadata, human input, agent responses, agent activity, provided context, exchanges, design implications |
| [viewer-arrival.md](viewer-arrival.md) | Implemented first-open slice: latest-exchange landing, role rules per harness, timing labels, collapsed agent activity, scoped search, saved views |
| [handoff-design.md](handoff-design.md) | The `slink view` handoff: session identity, saved previews, loopback server, background mode, SSH path, three journeys, per-harness contract matrix |
| [share-excerpt-v1.md](share-excerpt-v1.md) | Local excerpt composition v1: Share this view panel, drafts under ~/.slink/drafts, content-addressed preview, allowlist exporter, omission checks |

## Identity and encryption

| File | Summary |
|------|---------|
| [identity-encryption.md](identity-encryption.md) | Encrypted sharing: per-share AES-GCM keys, iroh-blobs ciphertext, accounts, recovery key, device approval and revocation, signed history protocol v1 |
| [named-recipient-sharing.md](named-recipient-sharing.md) | `/n/<id>` shares for verified emails: invitations with binding secrets, recipient claims, signed grants, key rotation, revocation, wire format |

## Capture and CLI internals

| File | Summary |
|------|---------|
| [spool-protocol.md](spool-protocol.md) | Frozen v1 contract for in-progress captures: `.spool` JSONL skeleton and spans, `.spool.pid` liveness, `.lock` commit mutex, `.corrupt` set-aside |
| [go-migration.md](go-migration.md) | 2026-07-18 decision record for moving the tap and CLI to Go, completed at v0.3.0 on 2026-07-19: frozen contracts, phases P0–P4, decisions, risks |

## Documentation outside docs/

| File | Summary |
|------|---------|
| [README.md](../README.md) | User-facing overview: install, `slink view` and sharing, privacy model, repository layout and development commands |
| [packages/pi-extension/README.md](../packages/pi-extension/README.md) | The pi extension: `/slink` and `/slink view` commands, install steps, exact-fidelity in-process capture and publishing |
| [packaging/README.md](../packaging/README.md) | The four CLI distribution channels driven from a tag: goreleaser (archives, packages, Homebrew cask), curl installer, npm, GitHub Release |
| [assets/brand/README.md](../assets/brand/README.md) | Elision mark, wordmark and brand colors; the social preview card and its export; brand CSS and icons copied into the Go embed and the server |
| [assets/product/README.md](../assets/product/README.md) | Product screenshots from fictional sessions and the README's How it works recording: sizes, what each shows, how to regenerate |
| [assets/logos/README.md](../assets/logos/README.md) | Provenance and license of the Hermes Agent logo; the other four harness logos have no note yet |
| [testdata/format/README.md](../testdata/format/README.md) | Format and scan cases every validator and scanner must agree on: the fields of the two files and how to change them |
| [testdata/protocol/README.md](../testdata/protocol/README.md) | Envelope, statement, history, vault and key cases every implementation must agree on: the fields, the generator, how to change them |
| [testdata/viewer/arrival/README.md](../testdata/viewer/arrival/README.md) | Fictional arrival fixture: eight exchanges, a recovered failure, Markdown, reasoning parts, standalone tool evidence, a child-agent result |
| [testdata/share/research/README.md](../testdata/share/research/README.md) | Synthetic research handoff fixture; the `OMITTED_INTERNAL_STRATEGY` marker must be absent from every exported channel |
