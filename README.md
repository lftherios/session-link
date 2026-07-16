# slink — the open client & format for [session.link](https://session.link)

Capture LLM sessions locally, publish the ones worth sharing. This repo is the
**open** half of session.link: the `slink` CLI, the `session/v0` format, and the
viewer. The hosted service is separate.

```bash
npx session.link dev -- python agent.py   # record its LLM calls (local only)
npx session.link open                     # browse captures, publish from the page
npx session.link push                     # validate → secret-scan → shareable URL
npx session.link login                    # sign in (GitHub)
```

## Packages

| Package | What |
| --- | --- |
| [`session.link`](packages/cli) | the `slink` CLI — a recording proxy, importer, local viewer, and publish flow. Zero-dependency bundle + standalone binaries. |
| [`@session-link/format`](packages/format) | the open `session/v0` format: TypeScript types, JSON Schema, and `validateRun`. |
| [`@session-link/viewer`](packages/viewer) | the React component that renders a `session/v0` document as an interactive trace tree. |

## The `session/v0` format

An open, framework-neutral representation of an LLM session: a flat list of
typed spans forming a tree, plus metadata, scores, and an attachment manifest.
Open-world by design — unknown span types, content parts, and roles validate and
round-trip. Schema: [`packages/format/session.schema.json`](packages/format/session.schema.json).

## Develop

```bash
npm install
npm test              # CLI + format + viewer tests
npm run build:cli     # bundle the CLI to one file
npm run build:binary  # standalone per-platform binaries (needs bun)
```

Distribution is documented in [`packaging/README.md`](packaging/README.md).

MIT licensed.
