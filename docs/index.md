# Documentation index

One row per document, each with a summary under 150 characters. `CLAUDE.md`
includes this file with `@docs/index.md`, so it is always in agent context.
When a doc is added, removed or renamed, update its row here in the same change.
Documents in `docs/` are dated working documents; the date line under each
title says when it was written and whether it describes a decision, a design
or an implemented slice.

## Product direction

| File | Summary |
|------|---------|
| [product-plan.md](product-plan.md) | Product promise and thesis, the three handoff use cases, six milestones in dependency order, and the integration TODO; draft dated 2026-09-12 |
| [success-criteria.md](success-criteria.md) | The nine agreed requirements: install, daemon, integrations, viewer, data control, sharing, trustworthy record, no disruption, recipient value |
| [kpis.md](kpis.md) | Proposed measurable targets and measurement definitions per criterion plus a week-two retention goal; proposed targets, not measured baselines |
| [project-review.md](project-review.md) | Review of the 2026-09-11 checkout: CLI, format, viewer and test state, fixes made during the review, and the next viewer priorities |

## Viewer and sharing experience

| File | Summary |
|------|---------|
| [viewer-model.md](viewer-model.md) | Agreed viewer content model: session metadata, human input, agent responses, agent activity, provided context, exchanges, design implications |
| [viewer-arrival.md](viewer-arrival.md) | Implemented first-open slice: latest-exchange landing, role rules per harness, timing labels, collapsed agent activity, scoped search, saved views |
| [handoff-design.md](handoff-design.md) | The `slink view` handoff: explicit session identity, saved previews, loopback server, background mode, SSH path, and three annotated journeys |
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
| [go-migration.md](go-migration.md) | 2026-07-18 decision to move the tap and CLI to Go: frozen contracts (format, spool, secret patterns, CLI surface, server API), phases and gates |

## Documentation outside docs/

| File | Summary |
|------|---------|
| [README.md](../README.md) | User-facing overview: install channels, quickstart, importer table, privacy model, components, roadmap and contributing |
| [packages/pi-extension/README.md](../packages/pi-extension/README.md) | The pi extension: `/slink` and `/slink view` commands, install steps, exact-fidelity in-process capture and publishing |
| [packaging/README.md](../packaging/README.md) | The four CLI distribution channels driven from a tag: goreleaser archives, curl installer, npm binary channel, Homebrew tap |
| [assets/brand/README.md](../assets/brand/README.md) | Excerpt mark and wordmark; `build:viewer` copies brand CSS and icons into the Go embed; the server repo keeps matching copies |
| [assets/product/README.md](../assets/product/README.md) | Product screenshots captured from fictional sessions, with sizes and the product state each one shows |
| [assets/logos/README.md](../assets/logos/README.md) | Provenance and license of third-party harness logos |
| [testdata/viewer/arrival/README.md](../testdata/viewer/arrival/README.md) | Fictional arrival fixture: eight exchanges, a recovered failure, Markdown, reasoning parts, standalone tool evidence, a child-agent result |
| [testdata/share/research/README.md](../testdata/share/research/README.md) | Synthetic research handoff fixture; the `OMITTED_INTERNAL_STRATEGY` marker must be absent from every exported channel |
