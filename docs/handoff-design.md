# CLI to web: first implementation

2026-09-11 · implements the first slice of the [product plan](product-plan.md).
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
diagnostics go to stderr. Browser launch failure leaves the URL usable.
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

| Harness | Invocation into local web | Explicit identity | Capture available | Evidence and limits |
| --- | --- | --- | --- | --- |
| Claude Code | `slink view --from claude-code` | Session ID or transcript path | Reconstructed transcript; proxy capture when recorded | File discovery, subdirectories and import fixtures tested. Native command integration remains open. |
| Codex | `slink view --from codex` | Header `id` / `session_id`, or rollout path | Reconstructed transcript; proxy capture when recorded | Both header spellings and independent sessions tested. A dedicated in-harness entry remains open. |
| pi | `/slink view` or common CLI | Current persisted transcript path | Reconstructed history; live SDK capture fallback | Bridge tested with resumed history and a new live turn; real pi runtime pilot remains open. |
| opencode | `slink view --from opencode` | SQLite session ID | Reconstructed messages and tool parts | Database discovery and pinned loading tested against fixture schemas. Structural patch events are not a saved final diff. |
| Hermes | `slink view --from hermes` | SQLite session ID | Reconstructed messages | Experimental; fixture schema verified. Live installation compatibility remains open. |

Prompts, outputs and tool evidence use the existing importers and their golden
fixtures. Fidelity labels remain visible. None of these adapters establishes
exclusive ownership of working-tree changes. No final-diff guarantee is added.
Discovery lists up to 30 native candidates, while an explicit ID can reach an
older session. The picker is a startup list; restart to discover newly created
sessions. An unreadable database is surfaced without hiding readable adapters.

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
[excerpt contract and verification](share-excerpt-v1.md). Excerpt publishing
remains gated. Richer document rendering and saved diffs remain separate milestones.
