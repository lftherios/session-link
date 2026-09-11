# Viewer arrival

Implemented locally on 2026-09-11, following the agreed first-open design.
No hosted deployment is part of this slice.

The [viewer content model](viewer-model.md) is the guiding product reference.
This document describes the implemented slice; its “supporting steps” label
is a presentation of agent activity, which can itself be the focus of inspection.

## Opening a session

The default reader starts on the latest exchange in the main conversation.
A user prompt and its associated response form the reading unit. Consecutive
prompt messages within the same recorded call stay together. Earlier assistant
steps, tool arguments/results and recorded failures remain in supporting steps.
Subagent exchanges remain navigable and do not replace the main landing point.

Each exchange has one collapsed **Agent activity** section. Captured thinking
and tool work live there even when the harness packages them in the same message
as the answer. Expanding the section reveals the thinking directly, without a
separate closed disclosure for every reasoning block. Search and activity links
open this section when needed. Answer selection addresses only its source parts;
reasoning and tools remain separately includable without changing captured data.

Unavailable reasoning is explained once inside that section. The Codex importer
preserves readable summaries or content when present; encrypted-only events now
use an empty `thinking.text` with `unavailable: true` and `reason: "encrypted"`.
Events with no text use `reason: "not_recorded"`. These are optional fields in
the open content-part format, not substitute reasoning text. Ciphertext is not
copied into normalized content. Older Codex imports containing the exact
`[reasoning]` placeholder receive the same treatment in the reader and selection
catalog. Placeholder-only rows are omitted, readable steps keep their source
addresses, and unavailable text cannot be selected for export.

The current format does not consistently preserve final-response channel
markers across harnesses. The default is therefore the latest recorded assistant
message containing text in that exchange. It is labeled **Latest response**,
without claiming that the work is complete or verified. A trailing prompt shows
**No response captured**. Recording status is shown only when explicitly present
in the saved metadata. Historical errors do not override later exchanges.

A meaningful existing name is used as the title. Recognizable placeholder IDs,
filenames and empty names fall back to a short task-prompt excerpt, then a neutral
source/date label. No model call or naming form is needed to start reading.
**Edit title** is available for local saved previews. Names persist in private
`drafts/titles/<source-id>.json` sidecars and seed new excerpt titles. Renaming
does not rewrite captured bytes or change an existing share draft's title.
Publishing the original whole session retains that original document's title.

## Action hierarchy and navigation

The header presents the title and one primary **Share this view** action at the
top right. A wide, central search bar is always visible. **This view** is the
default scope: the focused exchange, including its recorded activity, or the
conversation when browsing that view. **Whole session** also searches other
exchanges, subagents and recorded span payloads. Results identify individual
matching content and reveal the relevant response, context or activity.

Previous/next arrows and an exchange-position dropdown form a compact navigation
row beneath search. The dropdown opens the conversation outline; **Conversation**
switches to browsing. Search and navigation do not change the selected material.

Explicit `#message=<unit-prefix>`, `#exchange=<exchange-id>` and existing
`#span=<id>` links take precedence over saved reading state. Message/span links
open the readable exchange and reveal supporting material when needed. A span
with no conversational representation opens an inspector instead.

Previous/next controls navigate main exchanges; the outline includes subagent
exchanges too. Conversation mode
opens around the active exchange and loads earlier/later sections on demand.
Opening inspection uses a keyboard-accessible modal and retains the underlying
reading position. Closing it returns focus to its trigger.

The browser remembers exchange, mode and scroll position for the saved document.
Local previews use their content-addressed ID; inline documents use a reading
fingerprint. Only coordinates and identifiers go into reading-state storage.
Blocked browser storage does not prevent reading. A replacement document resets
component state. Metrics and the existing trace/raw viewer are under **Session details**.

## Selection, annotation and sharing

Content-level **Select** controls appear on hover and keyboard focus, and remain
available on touch devices. Selected content keeps its indicator while navigating.
A compact selection bar then offers **Annotate**, **Edit view** and **Clear**.
The content's small action menu contains recorded details and a precise link.
Session title editing lives under **Session details**.

**Share this view** opens an in-place panel. With no explicit selection it starts
with the visible exchange's human input and response text, opening on the input.
A trailing failure is included when present. In conversation mode it uses the
inputs and answers from that conversation. An explicit selection supplies the
panel instead. Agent activity is separately includable; collapsing or expanding
it in the reader does not change export scope. Unsupported selected attachments
are identified rather than silently represented as included.

The panel lists included material, permits removal and changing the starting
point, and provides an exact source-passage selector. Source offsets continue
to address original text, not rendered Markdown DOM text. Available source
reference cards can be explicitly included. Title and annotation fields are
secondary, initially collapsed unless opened through the selection actions.
Annotations never rewrite the recorded conversation.

**Save locally** writes a distinct immutable view and offers **Open saved view**
and **Download view**. **Preview view** opens that saved document. Saved views
remain listed in the share panel after reload and server restart. Local edit
provenance retains each view's own selection, title and annotation; reopening
its editor does not substitute the latest unrelated working draft. Preparing or
saving a view does not overwrite the older composer's working draft.

Preparations are retained while the session reader remains open, including when
closing the panel or navigating. Save explicitly to retain a view after reload.
The saved presentation currently includes a starting point and source-ordered
context; custom ordering and expanded/collapsed state remain future work.
Hosted excerpt publishing remains disabled pending hosted compatibility checks.

Existing excerpt recipients start with author context and the sender's chosen
material. The generic session reader labels its prompt neutrally as
**Original prompt** when not in a local viewer.

CommonMark/GFM rendering provides headings, tables, lists, code, links and
footnotes. Raw HTML is escaped. Links are restricted to HTTP(S), mail and fragment
targets; relative filesystem paths remain text. Supporting source-reference cards
remain visible and can resolve links in another included excerpt piece.

## Verification

SSR regressions cover action hierarchy, latest exchange selection, trailing prompts, recovered
errors, subagent ordering, standalone/replayed tool evidence, meaningful titles,
source-addressed share actions, Markdown structure and inert HTML/URLs. Go tests
cover local title persistence, source/draft immutability, independent saved views,
exact download bytes, saved-view recovery and guarded title writes.

`scripts/check-viewer-browser.mjs` exercises the built CLI with the fictional
`testdata/viewer/arrival/session.json` fixture, an isolated home and browser profile,
and an inactive local publishing target. It checks desktop/mobile arrival, explicit
links, saved reading position, scoped search, detail navigation, hover/keyboard
selection, annotations, exact passages, saved-view discovery, and exact recipient
content. Set
`SLINK_BINARY` and `BROWSER_BINARY` to override the default local binary and Brave
paths. Screenshots are written into its printed temporary artifact directory.

Remaining product work includes saved diffs, live updates to immutable snapshots,
native final-response signals where available, hosted integration and real colleague
pilots. The local viewer is not evidence of a completed hosted handoff.
