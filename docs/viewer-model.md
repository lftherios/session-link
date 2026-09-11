# Viewer content model

Agreed product guidance · 2026-09-11.

Use this model when designing the viewer's language, layout and behavior. It
describes what the viewer represents; it is not a wire-format specification or
a claim that every harness captures every kind of content. The
[product plan](product-plan.md) explains the CLI-to-web sharing experience, and
[viewer arrival](viewer-arrival.md) records the current implementation.

## Session and metadata

A **session** is the container for the captured conversation, agent activity,
provided context and associated metadata. A saved session represents what was
captured at a particular moment. It can be incomplete, and the work may continue
after that capture.

The **title** is session metadata, alongside source harness, project, dates and
capture status. It helps someone recognize the session; it is not a separate
content category. A title should help people start reading without requiring
them to name the session first.

The **complete session** means all the material in the available capture. It
does not imply that every event or all of the agent's context was recorded.

## Recorded content

| Content | Meaning | Examples |
| --- | --- | --- |
| **Human input** | What a person contributes to the interaction. | Initial prompts, follow-up questions, corrections, approvals and supplied material. |
| **Agent responses** | Messages from the agent addressed to the person. | Answers, explanations, recommendations and clarification questions. |
| **Agent activity** | Recorded work performed by the agent. | Tool calls and results, progress updates, retries, subagent work and captured reasoning. |
| **Provided context** | Instructions or background explicitly made available to the agent, with their recorded source and scope. | System instructions, repository guidance and supplied reference material. |

“Human input” is broader than “prompt.” “Response” does not mean final answer,
successful result or completed work. Activity can fail, be interrupted or have
no subsequent response. It remains inspectable in its own right, particularly
for debugging and review.

Preserve the source's distinctions where they exist. If a harness does not
identify whether assistant text is progress or a response, retain that
uncertainty rather than inventing a definitive classification. Only captured
reasoning can be shown; missing material is not evidence that no work occurred.

## Relationships and groupings

An **exchange** is a derived reading unit connecting human input, related agent
activity and agent responses. It can contain multiple messages and many steps,
or have no response yet. Do not require a one-prompt, one-answer structure.
Keep the originating agent and thread identifiable when work branches.

Associate tool attempts with their results when that relationship is recorded.
A missing result remains missing; it must not be presented as success.
Attachments and other materials remain associated with the content that
introduced them, without becoming duplicate items in a second context bucket.

“Context” has two distinct meanings:

- **Provided context** is actual information made available to the agent. When
  a person supplies a document in a message, that message remains human input
  and its attachment can serve as provided context without being duplicated.
- **Relevant context** is a relationship: existing material helps explain
  another piece. An earlier prompt, response or tool result keeps its original
  identity when someone includes it as context for a finding.

Additional explanation written by the sender while preparing a share is an
**author note**. Keep it distinct from the recorded session and from context
that the agent actually received. See the [excerpt contract](share-excerpt-v1.md)
for the current treatment of verbatim source content and author additions.

## Implications for viewer design

- Keep three levels clear: the session container, its content, and relationships
  or groupings that help someone understand that content.
- Treat focused reading, the conversation and complete inspection as views of
  the same captured material. Opening detail should preserve a person's place.
- Preserve access to human input, responses, activity and available context.
  Activity may be visually secondary during reading and central during debugging.
- Use one collapsed agent-activity section per exchange. Keep captured thinking
  within that section, including when the source message also contains the answer.
- Explain unavailable reasoning once per exchange. Empty or encrypted records
  indicate a capture limitation; they must not appear as repeated text placeholders
  or selectable evidence. Keep any readable summaries and other activity accessible.
- Keep provenance, missing content and capture limitations understandable.
  Do not imply a response is final or a capture is complete without evidence.
- Use the same concepts across harnesses while preserving differences in what
  each source records.

## Views and actions

A **view** is a derived presentation of session content: its explicitly included
material, starting point, presentation choices, title and optional author note.
Several views can draw on the same session. Saving a view preserves it locally;
sharing makes a version available to someone else. Ordinary reading position
can be remembered automatically without asking someone to create a named view.

The action vocabulary is **navigate, search, select, edit, annotate, save and
share**. Inspection is navigation into greater detail. Actions operate on the
content model; they are not additional categories of recorded content.

The viewer's priorities, agreed on 2026-09-11:

1. Land on the latest human input and associated answer.
2. Make search central and prominent, with explicit **This view** and
   **Whole session** scopes.
3. Put **Share this view** at the top right. Offer local saving within that flow.
4. Integrate exchange navigation compactly around the reading experience.
5. Reveal selection controls when someone hovers over content, with keyboard
   focus and touch equivalents. Selection persists while navigating.
6. Offer annotation and editing after selection, keeping them secondary to
   reading, finding and sharing.

Opening, expanding or collapsing content changes presentation. Explicit
inclusion determines what is saved or shared. The share panel must make that
scope reviewable; hidden content must not silently become part of an export.
Captured source text remains verbatim. Editing affects authored fields and
selection boundaries; annotations remain distinct from recorded content.

See [viewer arrival](viewer-arrival.md) for the current implementation and its
limits. In particular, a saved view currently preserves included text, a starting
point, title and annotation; arbitrary ordering and expansion settings are not
yet saved as presentation options.
