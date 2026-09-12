# session.link product plan

Working draft · updated 2026-09-12 · based on the planning conversation.

The [success criteria](success-criteria.md) record the nine agreed requirements:
easy installation anywhere, a tiny and efficient daemon, current and dependable
harness integrations, an inviting viewer, user control of data, and simple
sharing, plus trustworthy records, uninterrupted agent work, and immediate
recipient value. The [KPI scorecard](kpis.md) proposes targets and measurement
methods. Encrypted sharing with chosen peers remains an open design question.
Use these criteria to prioritize the work below.

The [viewer content model](viewer-model.md) is the agreed reference for session
metadata, human input, agent responses, agent activity, context and exchanges.
It also records the agreed view concept, action vocabulary and viewer priorities.

Execution update: CLI-to-web handoff and local excerpt composition are
implemented in this checkout. See [handoff design and verification](handoff-design.md)
for the journeys and harness matrix, and the [excerpt contract](share-excerpt-v1.md)
for selection, author context, saved drafts, recipient preview and omission
checks. Excerpt publishing is gated pending hosted-service verification.
The [viewer arrival slice](viewer-arrival.md) adds the latest-exchange landing,
local titles, compact navigation, prominent scoped search, hover selection,
annotation, local saved views and Markdown document rendering. Saved diffs and
real handoff pilots remain open.

Integration TODO:

- [ ] Build the [DeepSeek Harness (`dsh`)](https://github.com/deepseek-ai/deepseek-harness)
  integration. Verify its session storage and active-session identity against a
  pinned version; add discovery and import support that preserve prompts, outputs,
  tool calls/results, and source provenance through the common handoff contract.
  Add realistic fixtures and verify a live session from discovery through local
  viewing, excerpt preparation, and whole-session publishing. Document supported
  versions and limitations in the harness matrix. The landing-page logo has no
  status label; this integration remains unbuilt until these checks pass.

**Product promise:** Move work from any CLI agent harness into a polished,
shareable web experience, with the prompt, context, and evidence a colleague
needs to understand it and respond.

**Strategic thesis:** The proposed edge is the quality and consistency of
the transition from CLI work to the web across harnesses. Debugging, code
review, and research sharing are the initial uses of that transition.
Validate this direction through real handoffs and repeat use.

The unit being shared can be a selected outcome, a failure, a set of code
changes, or a whole session. The recipient should immediately understand
why the author shared it and where to begin.

Agreed direction:

- The initial audience is people sharing CLI agent work with a colleague.
- The central advantage to pursue is a smooth transition from CLI to web
  across harnesses, with a consistent recipient experience.
- Debugging, code review, and research sharing are all in scope.
- Code review includes inspecting diffs.
- Research sharing includes a selected portion of the outcome, its prompt,
  and additional context from the sender.
- Session discovery, invocation from the harness, web preparation, and the
  recipient viewer form one product experience and must be designed together.

There are two transitions to get right:

- **Sender:** from the session they are working in to the correct local web
  draft, with useful material ready to review and minimal repeated setup.
- **Recipient:** from an ordinary link to an understandable result and its
  evidence, without installing the originating harness or learning its logs.

The open format and shared viewer support this direction. Integration
quality, accurate session identity, dependable capture, and a polished web
experience make the promise useful in practice. Count a harness as
supported when users can complete verified handoffs through it.

Working assumptions to revisit after trying the first prototype:

- Selected source excerpts remain verbatim. An editable author note sits
  alongside them, clearly distinguished from the original conversation.
- A share includes only the selected material and explicitly included
  context. The full session remains available locally.
- Recipients respond through their existing conversation or review system,
  using precise links back to the shared material.
- The current hosted flow uses unlisted links. Make that audience explicit
  while investigating a private sharing model for chosen peers, including
  encryption, as recorded in the success criteria.

| Handoff | Recipient starts with | Supporting material | Successful outcome |
| --- | --- | --- | --- |
| Debugging | The question and observed failure | Relevant prompt, error, command and result, surrounding turns, applicable diff | The colleague can locate the failure and explain a likely cause or concrete next investigation. |
| Code review | The review request and saved diff | Task, constraints, relevant decisions, recorded test results | The colleague can assess the changes and point to a specific concern or approval. |
| Research | The selected finding or output | Original prompt, scope, captured citations, author's context | The colleague can understand and assess the finding, follow its sources, and use or discuss it. |

Debugging does not require a diff: a bad response, failed tool call, or
incorrect assumption can be the relevant evidence. It shares the review
workflow's need to move between an observation and its context.

The core experience should follow one sequence:

1. Invoke session.link from the current harness, or through the common CLI
   entry point. Pass explicit session identity when available. If selection
   is ambiguous, show a small, recognizable picker; do not silently equate
   the newest session with the active one.
2. Open a local web draft of that session, preserving the intended turn or
   output when the integration can identify it. Avoid file-path hunting,
   manual exports, or re-running the work as routine requirements.
3. Select the result, passage, failure, or changes to share. Include the
   relevant prompt and any additional evidence. Suggested
   context is visible and removable before publishing.
4. Optionally add a title and a short note explaining what the recipient
   should do.
5. Preview exactly what will be published, including downloads and any
   expandable context.
6. Publish deliberately and return the stable link in the browser and to
   the invoking terminal where supported. Copy it when possible and report
   the actual result. Authentication or a recoverable failure must preserve
   the prepared draft.
7. The recipient reads the chosen starting point, inspects included
   evidence, and links to a particular passage or change.

Target one invocation to reach a useful web draft after setup. Initial
installation and sign-in are separate parts of onboarding to evaluate.
A simple whole-session share uses sensible defaults and can proceed
directly from preview to publish. Selection and author context should add
value without becoming mandatory preparation for every share. The three
use cases guide the design and tests; users need not choose a category to
begin.
For remote or headless CLI use, provide an explicit browser connection path
and actionable fallback; an automatically opened localhost URL is not a
complete solution when the session runs on another machine. The transport
design remains a milestone-1 decision. Opening a local draft does not
publish its contents.

The opening page should present the author's note, the selected material,
and routes into supporting context. Research should read like a clear
document; code changes should render as a proper diff. Full transcript and
raw inspection remain available for the material included in the share.
Moving into detail and back should preserve selection and reading position.

The current checkout provides a useful foundation: five importers, local
capture, explicit publishing, a shared React viewer, span links, immutable
published documents, and an extensible format. The main gaps are:

| Gap | Evidence in this repository | Implication |
| --- | --- | --- |
| Entry from the harness | A dedicated pi extension exists; the common CLI discovers and imports sessions across five harnesses. | Define one handoff contract and the most direct supported invocation for each harness. |
| Active session identity | Generic discovery selects by recency and project directory. | Concurrent sessions, resumed work, and subdirectories need tests and a clear ambiguity flow. |
| CLI-to-browser continuity | Local viewing and publishing exist, but selected-output transfer and a composed draft are absent. | The handoff must preserve the user's intended material and survive authentication or upload failure. |
| Preparing a share | Local publishing reads and uploads the capture file as a whole. | Selecting content requires creating a separate export, beyond changing what the viewer displays. |
| Author context | No dedicated composition flow for a handoff note and selected outcome. | The author currently has to explain the link elsewhere. |
| Complete inspection | Some standalone tool results are absent from transcript mode; switching views loses reading state. | A colleague can miss evidence or lose their place. |
| Research presentation | Markdown handles code and links, with limited support for document structure. | Research tables, lists, headings, and citations need a deliberate reading experience. |
| Saved code changes | Tool arguments may contain patches, but there is no standard changeset representation; the opencode importer skips structural patch events. | A trustworthy final diff needs an explicit source and baseline. |
| Evidence of usefulness | We have not yet reviewed actual recipient feedback or recurring usage. | Progress should be evaluated through real handoffs as well as implementation checks. |

Proposed milestones, in dependency order:

**1. Design the complete CLI-to-web journey with the three use cases.**

Choose one debugging example, one code review, and one competitor-research
excerpt. For each, record the sender's question, the primary material,
required context, excluded content, and what the recipient needs to answer.
Sketch invocation inside the harness, session resolution, local browser
arrival, preparation, publishing, and the recipient page. Exercise at least
two harnesses with different integration capabilities. Check the hosted
service contract and the local-versus-remote browser path during this
design, before committing to a transport or export format.

Deliverable: three annotated examples, a harness capability matrix, and one
common interaction design. The matrix records invocation support, explicit
session identity, capture fidelity, available content and diff data, and
local/remote limitations for each integration.
Exit condition: each journey has an identifiable source session, a useful
web arrival, and a finite package of evidence for the recipient.

**2. Establish a dependable common handoff from the CLI to the browser.**

Define the integration contract: source harness, explicit session identity
when available, project/repository context, capture capability, and optional
selected-turn or artifact references. Use the common CLI to resolve and
prepare the draft, with thin harness-specific entry points where supported.
The same interaction may use different invocation syntax across harnesses.

Prefer the current session when identified by the integration. Support
resumed sessions and historical imports; show a recognizable choice when
identity is uncertain. Preserve a stable draft snapshot if the agent keeps
working. Avoid making always-on recording a prerequisite for sharing.

Deliverable: a reliable local web arrival through two representative
integrations, plus the common CLI fallback. Existing explicit publishing
commands retain their documented meaning; distinguish entering a draft
from publishing it.
Exit condition: one invocation after setup opens the correct session for
local use. Tests cover simultaneous sessions, resumed work, a live session,
and browser-launch failure. The remote/headless path has explicit tested
instructions or is clearly identified as unsupported for that release.

**3. Build selection, author context, and an exact export preview.**

Allow the sender to select a response, passage, or tool result and include
its prompt plus additional context. Support non-adjacent selections so a
research finding can travel with an earlier constraint. Add the author note
and preview the exported document using the recipient's viewer.

Create a separate share document and leave the local source intact. Mark
excerpts, omissions, and explicit redactions. Suggested supporting material
must be visible to the sender. Retain relevant citation targets when a
selection would otherwise leave a dangling reference.

Deliverable: local creation and preview of a research share and a debugging
share using existing captured data.
Exit condition: excluded content is absent from the exported document,
raw data, metadata, attachments, and downloadable payloads. An omitted
prompt or tool result is described as unavailable instead of invented.

**4. Make the included evidence readable and inspectable on the web.**

Complete tool-call/result rendering, provide full search over the included
material, preserve position during inspection, and support links to
specific excerpts. Add Markdown headings, lists, tables, and usable source
links. Make the same examples readable on narrow screens and with keyboard
navigation. Label recorded test output distinctly from an assistant's claim
that tests passed.

Deliverable: complete research and debugging reading experiences.
Exit condition: a recipient unfamiliar with the original session can
identify the task, understand the selected outcome or failure, and find the
supporting evidence without the sender walking them through the interface.

**5. Add dependable code-review artifacts.**

Capture a saved set of changes from an explicitly identified Git baseline
and target state, or a sender-supplied patch with its origin recorded.
Show which committed, staged, unstaged, and untracked changes are included.
The sender reviews that scope before sharing. Preserve the published
snapshot even if the repository later changes.

Provide file navigation, additions and deletions, line context, unified and
side-by-side views, and stable links to changed lines. Renames, deleted files,
binary files, and unavailable context need explicit representations.
Connect changes to session events where the captured evidence supports the
connection; label suggested connections as such.

Do not infer that all working-tree changes belong to one session. Existing
edits, human intervention, multiple agents, and later changes make that
attribution unreliable. Showing edits attributable exclusively to a session
requires reliable checkpoints or equivalent evidence and is a later
capability. Tool-call patches alone do not establish the final repository
state, because calls can fail and changes can be reverted.

Deliverable: a share whose main artifact is a saved diff with supporting
session context.
Exit condition: the displayed diff agrees with the selected baseline and
target, including a case with pre-existing edits and a case with a reverted
agent change. Missing change data produces an honest unavailable state.

**6. Verify consistency across harnesses and pilot all three handoffs.**

Complete hosted integration checks before enabling the new export flow.
Confirm that preview, published rendering, and downloads
use the same selected content and that the existing validation, secret
scanning, ownership, and deletion behavior apply to the new documents.

Extend the handoff to all five currently supported harnesses and report
verified capabilities individually. Run common semantic and interaction
fixtures across adapters: prompts, outputs, tool results, source links,
subagents, and changes should carry consistent meaning where present.
Absent data must be explicit. A missing diff must not block sharing a
research outcome. New harnesses should enter through the same contract and
acceptance suite.

Use two real exchanges per handoff type as an initial qualitative pilot,
covering more than one harness. Measure correct-session selection, time
from invocation to a useful browser draft, manual recovery steps, and time
to a published link. Also observe requests for missing context, whether the
recipient can answer the intended question, and whether senders choose the
tool again. Track initial setup separately from repeat use and compare the
friction across harnesses. Agree numerical targets after observing the
baseline; this pilot is too small to establish broad demand.

Deliverable: findings and a revised priority order based on actual use.
Exit condition: repeat the handoffs after fixing the largest obstacles and
identify which use case deserves the next investment.

Implementation boundaries for estimating the work:

- Keep the existing full-session flow working. New shares are independent
  immutable documents with stable anchors and explicit provenance.
- Prototype author context and selection metadata using the format's
  existing versioned extension mechanism. Define how excerpted messages
  and diffs validate before choosing the final representation; do not claim
  that preserving unknown fields guarantees compatibility with the hosted
  viewer.
- Project the selected material into the exported document. Normalized
  messages, raw provider payloads, repeated histories, tool arguments, and
  attachment manifests can all contain copies of excluded content.
  Publication tests must inspect the actual outgoing bytes.
- Treat capture fidelity and share transformations separately: an exact
  original capture can yield an excerpted or redacted share.
- Keep harness-specific parsing and discovery behind the common handoff
  contract. The viewer should render shared concepts consistently, with
  explicit fallbacks for source-specific or unavailable data.
- Maintain a tested compatibility matrix and realistic fixtures as harness
  histories and extension APIs change. Automatic discovery and supported
  versions are ongoing product responsibilities.
- Add fixtures for all three handoffs and for repeated context, missing
  results, failed commands, redacted copies, and unavailable repository
  state. Include both semantic checks and browser interaction checks.
- The hosted service is outside this checkout. Access to its code or
  contract is a dependency for the publishing milestone. Go tooling is also
  needed to verify CLI changes.

Deferred until these exchanges demonstrate value: a general document
editor, automatic narrative generation, built-in comments, executing or
remixing sessions, comparisons across runs, and public discovery. Existing
communication tools can carry the first rounds of feedback through precise
links. Broad harness coverage is part of the strategy; prioritize new
integrations by actual usage and apply the same quality requirements.

Remaining decisions are deliberately visible: the native invocation for
each harness, remote/headless transport, the preferred Git baseline, how
much supporting context to suggest, whether author editing should later
extend to source outputs, and the access and encryption model for sharing
with chosen peers.
Resolve invocation and transport during the initial journey design;
prototype the remaining choices with the three examples. Calendar estimates
should follow that design and the hosted-service check.
