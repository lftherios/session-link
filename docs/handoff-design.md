# CLI to web: first implementation

2026-09-11 · implements the first slice of the [product plan](product-plan.md);
harness contract updated 2026-10-07.
These are development scenarios, not reports of real colleague exchanges.

## Common interaction

`slink view [--from harness] [--session ID-or-path] [--span ID]`
opens a local saved preview. An integration should pass the exact session path
or ID when available. IDs are scoped to the current project or its ancestors;
an explicit transcript path can refer to any project. A missing or ambiguous
ID errors instead of substituting another session. Without explicit identity,
multiple candidates open a searchable browser picker. `--pick` always opens it.

Selection pins the source identity. Opening its preview imports the currently
available data and saves a separate, validated `session/v0` document. While a
transcript file keeps its size and modification time, and slink itself is
unchanged, reopening reuses that document instead of importing again. The
browser renders that file and publishing sends those same bytes through the
existing validation and secret scan. A large session is saved twice: the
whole document, and a reading copy without the recorded results of tool
spans. The page carries the reading copy, which is what the reader shows on
arrival, and fetches the whole document behind it; publishing and sharing
always use the whole document. No source material is rewritten to add
annotations; materializing a live recorder's spool still updates its capture.
Later source changes and failed uploads do not replace a saved preview.

The server binds loopback on a free port and prints its ready URL to stdout;
diagnostics go to stderr. The complete URL includes a random per-launch access
key in its fragment. The browser sends that key in a private request header;
all session reads and actions require it. Keep the complete local URL private.
The key is scoped to this viewer process, survives local navigation and copied
links, and never enters a query string or a cookie. Browser launch failure
leaves the URL usable. Pages reject framing and send no referrer. Transcript
images embedded as raster data remain visible; external image URLs require an
explicit click and are never fetched merely by opening a session.
`--background` waits for that URL before releasing the calling harness and
stores diagnostics in `~/.slink/viewer-*.log`. Publishing returns the link in
the browser and the foreground terminal, or the background log. A background
pi invocation receives the local URL; it does not yet receive a later published
URL back in the TUI. Stop the viewer from the sessions page or with Ctrl-C in
foreground mode. Saved previews remain on disk and can be reopened by path.

For remote shells, use an explicit port and SSH local forwarding as documented
in the README. The same loopback URL then works through the tunnel. Automatic
forwarding and a hosted relay are outside this slice. Forwarding guidance and
headless behavior are tested; an actual two-machine SSH exchange remains to be
verified.

## Three annotated journeys

| | Debugging | Code review | Research |
| --- | --- | --- | --- |
| Sender's question | Why did the command fail, and what should I check next? | Does this change meet the task and avoid a regression? | Does this finding change which competitor we should investigate? |
| Primary material | Failed command, recorded error, original prompt | Saved diff and review request | Selected finding and a short author note |
| Required context | Relevant preceding turn and tool result | Task, constraints, baseline and recorded tests | Original research prompt, scope, captured source links |
| Excluded material in the future export | Unrelated later task | Unrelated edits and irrelevant conversation | Other findings and internal planning |
| Recipient's answer | A likely cause or next diagnostic step | A concern tied to a line, or approval | An assessment with a source or follow-up question |

**Debugging example:** open
`testdata/import/pi/error-and-trailing/input.session.jsonl` with `slink view`.
The failed turn and trailing prompt must survive import and remain inspectable.
In an active pi session, `/slink view` supplies its transcript path directly.
The preview initially contains the full session. **Choose what to share** opens
the local composer to select the failure and necessary context for an excerpt.

**Code review example:** open
`testdata/import/codex/basic/input.session.jsonl` with `slink view`.
The task asks for a health endpoint and then a test. A response claiming those
changes is available; a saved final diff and recorded test run are absent.
The viewer must not turn those claims into verified changes or test evidence.
The final journey will start with a separately captured diff and its chosen
baseline. This first slice supports inspecting and sharing the conversation.

**Research design example (synthetic):** the prompt is “Compare Atlas and
Beacon for self-serve onboarding; use their public documentation and identify
uncertainties.” The selected outcome is an onboarding comparison with its
captured URLs. The author note is “Please check whether this changes our trial
flow priorities.” The share should include that prompt and finding while
excluding a later internal planning discussion. These fictional names make no
claims about actual companies. This scenario now has a
[source, selection and expected export fixture](../testdata/share/research/README.md).
The local composer and recipient preview enforce those exclusions in the
generated document, including its raw view and download.

## Harness contract and current verification

| Harness | Invocation into local web | Explicit identity | Capture available | Checked against |
| --- | --- | --- | --- | --- |
| Claude Code | `slink view --from claude-code` | Session ID or transcript path | Reconstructed transcript; proxy capture when recorded | 2.1.292: field names and directory naming read from its program; not run live |
| Codex | `slink view --from codex` | Header `id` / `session_id`, or rollout path | Reconstructed transcript; proxy capture only through Codex's own configuration | 0.160.1: rollout item kinds from its source; routing run live against a mock provider |
| pi | `/slink view` or common CLI | Current persisted transcript path | Reconstructed history; live SDK capture | 0.67.68 and 1.0.4: session format and extension events from the packages; not run live |
| omp | `slink view --from omp` | Session ID or transcript path | Reconstructed transcript | 18.1.18: session format and directory rules from its source; run live against a mock provider |
| opencode | `slink view --from opencode` | SQLite session ID | Reconstructed messages and tool parts | 1.18.35: table definitions from its source; run live against a mock provider |
| Hermes | `slink view --from hermes` | SQLite session ID | Reconstructed messages | 0.15.1: schema and session-list rules from its source; not run live |

Evidence and limits for each:

- **Claude Code.** File discovery, subdirectories and import fixtures are
  tested. Project directories follow Claude Code's own naming, so paths with
  punctuation and paths past its 200-character cut are found, and
  `CLAUDE_CONFIG_DIR` is honoured. A response written as several entries
  imports as one call with its usage counted once. The summary written when a
  conversation is compacted, and background-task notifications, read as
  provided context. A sub-agent's transcript, kept in `subagents/` beside the
  session, is nested under the `Agent` call its meta file names; inline
  sidechain entries from older versions are still only counted. Native
  command integration remains open.
- **Codex.** Both header spellings and independent sessions are tested.
  Function, custom-tool (`apply_patch`), local shell, tool search and web
  search calls import as tool calls in recorded order; any other rollout item
  is kept verbatim in a custom span. `slink on` and `slink record` export
  `OPENAI_BASE_URL`, which Codex does not read, so they capture nothing from
  it and `slink record` says so. With an API key, setting `openai_base_url`
  in Codex's configuration to the tap's `/openai/v1` address does route its
  calls through the tap, once its WebSocket attempts fall back to HTTP, and
  they are recorded. ChatGPT sign-in is untested. Rollouts compressed to
  `.jsonl.zst`, which 0.160.1 does behind a flag that is off by default, are
  not read. A sub-agent thread is its own rollout that opens with a copy of
  its parent's history; it is nested under the `spawn_agent` call that
  returned its task path, read from where its own history starts. Messages
  between agents are kept, with a task's payload as Codex records it, which
  is encrypted content. A dedicated in-harness entry remains open.
- **pi.** The import is the branch that ends at the transcript's last entry,
  which is the one pi resumes; messages on branches the person went back from
  are counted in the metadata. Compaction and branch summaries read as
  provided context, an errored turn is marked as one, and a name given to
  the session is its title. `PI_CODING_AGENT_DIR`, and a session directory
  set through `PI_CODING_AGENT_SESSION_DIR` or `sessionDir`, are followed.
  `/slink` publishes the live capture when it holds the whole session, and
  pi's transcript when the session was resumed. The bridge is tested with a
  stub CLI; a real pi runtime pilot remains open.
- **omp.** oh-my-pi keeps pi's transcript, so the pi importer reads it; the
  session's title comes from the title line omp rewrites in place, and a
  tool's span starts when omp records that it started. Discovery follows
  omp's own rules: its store under `~/.omp` or `PI_CONFIG_DIR`, a profile
  from `OMP_PROFILE` or `PI_PROFILE`, `PI_CODING_AGENT_DIR`, data it has
  moved under `XDG_DATA_HOME`, and its directory names, which are relative
  to the home or temp directory where the project is inside one. A
  directory still carrying the older whole-path name is read too. A real
  18.1.18 run, in an isolated home against a mock provider, is the
  `omp/v18.1-run` fixture. A sub-agent's transcript, in a directory named
  after its session's file, is nested under the `task` call that reports it.
  A session directory given with `--session-dir` is not followed, and
  whether the pi extension loads in omp is untested.
- **opencode.** Database discovery and pinned loading are tested against
  fixture schemas, and `OPENCODE_DB` is followed. A real 1.18.35 run, in an
  isolated home against a mock provider, wrote its session to the `message`
  and `part` tables, which are the ones read; its text, reasoning, tool call,
  result and usage imported as recorded, and that session is the
  `opencode/v1.18-run` fixture. opencode's second store (`session_message`),
  for a newer session engine, stayed empty in that run and is not read. Text
  opencode wrote itself into a user message, such as its account of reading
  an attached file, reads as provided context. A sub-agent's work is a child
  session, nested under the `task` call that records its ID. Structural patch
  events are not a saved final diff.
- **Hermes.** Experimental. Rewound messages are left out, and content is
  structured only where Hermes marks it so. A continuation after context
  compression is found in its conversation's directory and imported as its
  own session, linked to its parent in the metadata; the two are not merged,
  because the continuation repeats part of its parent. A delegate's session
  is nested under the tool call that was running when it started, because
  Hermes links it to its parent session and not to a call. `HERMES_HOME` and
  the active profile select the store.

Prompts, outputs and tool evidence use the existing importers and their golden
fixtures. Fidelity labels remain visible. None of these adapters establishes
exclusive ownership of working-tree changes. No final-diff guarantee is added.

An image a person attached, and one a tool returned, import as image parts
that carry the image itself, so the reader shows them without fetching
anything: Claude Code's inline images, Codex's attached images, tool results
and generated images, pi's images, omp's from the blob store beside its
sessions, opencode's attached files, tool attachments and files in replies,
and Hermes's image parts. The `images` fixture of each harness covers it; the
omp and opencode ones come from real runs. An image whose bytes are not at
hand, such as an omp reference without its blob store, or whose kind cannot
be told, is kept as recorded, as data. Two limits remain. An exact capture,
from the tap or the pi extension, leaves an image's bytes in the raw request
and shows a note in its place. An excerpt cannot include an image yet.

A sub-agent's work is part of the session that delegated it. Every harness
keeps it apart, in a transcript, rollout or row of its own, and each import
reads it back and nests it as a child agent under the tool call that started
it, so the reader shows the delegated task and the work done on it where the
delegation happened. Sub-agents of sub-agents nest the same way. A preview is
made again when a sub-agent's transcript changes, not only its parent's. The
`sub-agent` fixture of each harness covers it; the Claude Code, Codex, omp and
opencode ones are real runs, reduced, and the Hermes one is written by hand.
An exact capture has no such structure: the tap records every call in one
flat sequence.

A session belongs to the directory it was started in. Discovery looks in the
current directory, then in each parent short of the home directory and the
filesystem root, for `slink view`, `slink import` and `slink share` alike; a
session started in the home directory is found only there. It lists up to 30
native candidates, most recently active first, while an explicit ID can reach
an older session. Sessions a harness records for its sub-agents are left out
of the list and are never chosen as the newest session: Codex sub-agent and
internal threads, opencode child sessions, and Hermes sessions started while
their parent was live. An explicit ID still reaches one. Captures follow the
same rule by the directory they record; the always-on tap serves every shell
and records none, so its captures are listed in every project. The picker is a
startup list; restart to discover newly created sessions. An unreadable
database is surfaced without hiding readable adapters.

## Validation and remaining work

Automated coverage includes concurrent sessions, exact IDs older than the
display limit, project subdirectories, file and SQLite stores, changing source
files and live spools, immutable preview bytes, failed upload retry, secret
blocking, cross-origin rejection, hostname checks, foreground/background
startup, browser launch failure, SSH guidance and occupied-port recovery.
The pi bridge is exercised with a stub CLI so tests cannot publish externally.
Browser checks cover picker search, real embedded rendering, unauthenticated
reload, mobile dark layout and stopping the detached server.

The outgoing document and `/api/runs` contract are unchanged and verified
against a local mock receiver. The hosted service is outside this checkout;
no live publication or hosted compatibility test was performed. Local selection,
author context and export projection are implemented in the next slice; see the
[excerpt contract and verification](share-excerpt-v1.md). Excerpt publishing was
gated at the time; the hosted service has accepted encrypted excerpts since
2026-09-12. Richer document rendering and saved diffs remain separate milestones.
