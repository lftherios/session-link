# Local excerpts: version 1

2026-09-11 · implementation of milestone 3 in the [product plan](product-plan.md);
checked against the code on 2026-10-09.
This checkout supports local composition, recipient preview, download and
publishing, as an encrypted link or to named recipients.
Hosted encrypted excerpt publishing has been live since 2026-09-12.

## Sender and recipient

The primary entry from a saved session is now **Share this view**, an in-place
panel with explicit inclusion, annotation, recipient preview, local saving and
publishing.
See [viewer arrival](viewer-arrival.md) for that interaction. The standalone
composer remains available through saved-view **Edit selection** links and
existing `/compose/<source>` routes, including older response/passage links.
Select whole content pieces or one contiguous passage per piece, explicitly
include an available original prompt, add a title and separate author note,
and choose the reader's starting point. The editor supports source search,
an included-only filter and pagination. Images, attachments and unresolved
external content references are marked unavailable for this text-based export.

Drafts save under `~/.slink/drafts/<source-preview-id>/`. Each save creates a
private, numbered revision using an atomic create; prior revisions remain on
disk. A stale tab or another viewer process receives a conflict instead of
overwriting a newer revision. Failed saves retain the editor's current state.
The source snapshot remains unchanged.

**Preview excerpt** creates a separate, content-addressed document. The reader
sees the author note, the chosen starting point, then other included context
in source order. Labels distinguish passages, omissions, missing prompts,
missing tool arguments and source capture fidelity. Each piece has a fragment
link. Raw data inspection and **Download excerpt** use only the exported file.
Editing a draft never changes a previously generated preview.

## What is exported

The Go exporter constructs a new `session/v0` document from a positive allowlist:

- Author title and note, original `created_at`, and an allowed original fidelity value.
- Selected text, readable recorded reasoning, errors, or the displayed JSON for selected
  tool arguments/results and data. Text passages retain the original characters;
  structured values use the JSON representation displayed in the editor.
- Generated span IDs, normalized role/kind labels and the excerpt metadata below.

Original source paths and IDs, raw provider payloads, repeated histories,
unselected messages, arbitrary metadata/extensions, model parameters, metrics
and attachment manifests are never copied from the source envelope. Content
inside a selected piece is included, so the sender still reviews that content.
UTF-16 selection offsets match browser text selection; invalid ranges and split
surrogate pairs are rejected rather than replacing characters.

Reasoning marked unavailable, empty reasoning text, and the legacy Codex
`[reasoning]` placeholder are unavailable catalog items. They cannot be exported
as readable evidence. Original source-part indices remain stable around these
items so selecting the adjacent answer still addresses its actual source text.

Reference-style Markdown needs its definitions. Available definitions are
offered as separate **Source reference** cards, allowing a citation to be
included without intervening private text. Export requires explicitly selecting
an available definition used by the excerpt. A reference absent from the source
is labeled unavailable. This is a conservative reference check, not a complete
Markdown parser. The reader uses CommonMark and GFM, including headings, lists,
tables and citations; separately selected source definitions resolve in excerpts.

## Wire contract

The document retains the existing `session/v0` schema. Its source is
`{kind: "import", label: "session.link excerpt", fidelity: "partial"}`.
Generated `custom` spans contain selected text; a separate author-role span
contains the note. This keeps the document inspectable by generic readers.

`extensions["session_link.share.v1"]` contains:

| Field | Meaning |
| --- | --- |
| `kind: "excerpt"` | Selects the excerpt reader. |
| `omissions: true` | The export does not represent the complete source session. |
| `original_fidelity` | Optional original capture fidelity, separate from selection completeness. |
| `note_id` | Optional ID of the author context span. |
| `primary_id` | Included span to display first. Always written, the first item by default; the reader requires it. |
| `items` | Source-ordered records with `id`, `kind`, `role`, `passage`, `omitted_before`, and optional `prompt_missing` / `tool_call_missing`. |
| `references_unavailable` | Some referenced definitions were unavailable in the original material. |

The source-to-export pointer needed by **Edit selection**, the view's own draft,
and its local save timestamp live only in
`~/.slink/drafts/exports/<export-id>.json`; they are absent from the downloadable
document. The source's share panel lists these saved views, with the link of any
view this viewer has published (`~/.slink/drafts/published/<export-id>.json`,
newest publish only). The editor restores
the requested view's draft rather than substituting another working draft.
A preview URL is local to the running viewer.

## Hosted integration boundary

The local publish endpoint accepts an excerpt like any saved preview: it is
validated, scanned for credentials, encrypted on this machine and uploaded.
The gate that rejected excerpt documents was removed on 2026-09-12, when the
hosted service began accepting encrypted shares. The server stores ciphertext
and cannot tell an excerpt from a whole session.

The record from 2026-09-11, when publishing was still gated: GitHub inspection
confirmed that `lftherios/session-link` had only `main`, at
`eb8888da69ce0e68c7617238a63b1fc53353bdea`, matching the checkout's base, and the
service's source and deployment configuration were not in the checkout. The
checks listed then as conditions for enabling excerpt publication were
acceptance and preservation of the extension, exact raw/download bytes, the
recipient renderer, link fragments, attribution and deletion against the
service.

## Verification and limits

The [fictional research fixture](../testdata/share/research/README.md) places an
omitted marker in source/raw/history/metadata/attachment channels. Tests compare
the complete generated document with the expected export and check the marker
is absent. Coverage includes UTF-16 boundaries, missing context and citations,
an import fixture from each harness except omp, draft restart/revision conflicts, immutable
previews, local request guards and exact upload bytes against a mock receiver.

`scripts/check-viewer-browser.mjs` exercises the share panel in a real browser:
passage selection, author comment, what a saved view contains, saved views after
reload, and the mobile dark layout. The standalone composer's explicit prompt
inclusion, fragment copy and download are not covered by a script in this
checkout. These checks do not establish live hosted compatibility, a real
colleague exchange, saved Git diffs, attachment support or parity with every
installed version of each harness.
