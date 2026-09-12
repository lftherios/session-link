# Product KPI scorecard

2026-09-12 · translates the nine agreed [success criteria](success-criteria.md)
into measurable outcomes.

These are **proposed initial targets**, not measured results or published
guarantees. Baselines have not been collected for this scorecard. Validate the
targets on representative environments before adopting them as commitments.
P95 means 95% of measured attempts finish within the stated time.

## Scorecard

| Criterion | KPI | Proposed initial target |
| --- | --- | --- |
| 1. Easy installation | Successful first installations; time from starting installation to the first usable session view | **≥99% success; ≤60 seconds P95.** One installation command, with no separate runtime installation. |
| 2. Tiny, efficient daemon | Idle resident memory; idle CPU; installed binary size; streaming overhead | **≤20 MiB idle RAM; ≤0.1% of one CPU core idle; ≤15 MiB binary; ≤5% added streaming duration P95.** |
| 3. Current harness integrations | Successful handoffs on supported versions; time to test a new upstream stable release; time to restore broken compatibility | **≥99% handoff success; new releases checked within 24 hours; regressions fixed within 72 hours.** |
| 4. Beautiful, responsive viewer | Time to a usable reading view; interaction latency; reader experience rating | **≤1 second local / ≤2 seconds hosted P95; ≤100 ms interactions P95; ≥80% of pilot users rate the reading experience 4/5 or better.** |
| 5. User control of data | Unintended session uploads; exported content matching the approved selection; understanding of who can read a share | **0 unintended uploads; 100% selection/export checks pass; ≥90% of pilot users correctly identify the audience.** |
| 6. Simple sharing | Successful publishing attempts; time from confirmation to a usable URL; required actions after choosing material | **≥99% success; ≤3 seconds P95; ≤3 required actions.** |
| 7. Trustworthy records | Wrong-session incidents; silent loss of available source content; fidelity checks | **0 wrong-session incidents; 0 silent-loss incidents; 100% fidelity fixtures pass.** |
| 8. No disruption to agent work | Recording sessions without a recorder-caused interruption; offline and failure-recovery checks | **≥99.9% interruption-free sessions; 100% of the defined offline and failure-recovery checks pass.** |
| 9. Immediate recipient value | Recipients who understand the takeaway and requested next action within a minute; successful recipient tasks without extra explanation | **≥90% understand within 60 seconds; ≥80% complete the agreed task without asking for missing context.** |

## Overall product outcome

**Week-two retention: target ≥40%.** Of users who first opened a usable real
session in a given week, at least 40% return to read, search, prepare, or share
a session during days 7–13 after that first use. A running background daemon
does not count as a return. Report viewing and sharing activity separately.

Also track **weekly useful handoffs** in the opt-in pilot: exchanges where the
recipient confirms they understood the material or completed the intended
task. Count a sender/recipient/shared-snapshot combination once, regardless of
reloads. Establish a baseline before setting a volume target. A link being
opened alone does not establish a useful handoff.

## Measurement definitions

- **Reference conditions:** version and retain the benchmark fixtures and
  environment descriptions. Start with a standard session of approximately
  1 MiB / 500 events and a large session of approximately 10 MiB / 10,000 events.
  Include long outputs, tool results, branching, and missing source data.
  Use a fixed reference machine, a representative phone, and a recorded
  network profile; initially use 50 Mbps and 50 ms round-trip latency for
  network-dependent targets. Publish results per environment.
- **Installation:** use a clean environment with a supported harness history
  fixture already present. Start the clock at the install command and stop
  when the intended session is readable and interactive. Include download,
  extraction, PATH setup, and invocation. Record user assistance and manual
  steps. Also measure upgrades, uninstall, and remote/container setup as
  separate journeys. Do not pool platforms or installation channels to hide
  one that is failing.
- **Daemon:** measure the recorder process separately from the viewer, agent,
  and browser. Sample idle resource use after warmup over 15 minutes. CPU is
  normalized to one core; RSS and binary size use MiB. Compare matched streams
  with and without recording against a controlled upstream, including ten
  concurrent streams. Calculate added duration per paired run, then P95.
  Track active peak memory, startup time, disk growth, and a 24-hour soak
  alongside the headline budgets; idle performance alone is insufficient.
- **Compatibility:** a successful handoff opens the intended session with the
  expected available content and completes the capability being tested. Test
  discovery, viewing, and publishing separately. Count failures on every
  declared supported harness/version combination, including resumed sessions
  and concurrent agents. Check real runtime histories as well as stored
  fixtures. The 24-hour clock starts at the upstream stable release; the
  72-hour clock starts at the first confirmed incompatibility and ends at a
  released, verified fix. Removing a support label does not count as a fix.
- **Viewer:** start at the local command invocation or hosted URL navigation;
  stop at useful content that responds to input, not an empty loading shell.
  Measure search, navigation, and expanding activity from input to the updated
  frame. Headline budgets apply to the standard fixture; initially allow twice
  the time for the large fixture and report it separately. Collect the 1–5
  reading rating after the same realistic task, with comments about what
  worked or frustrated people. Keyboard and mobile task completion are
  required checks alongside the rating.
- **Data control:** an unintended upload means session content or metadata
  reaches the sharing service without the user's publishing action. Normal
  agent-to-model traffic is a separate activity. Inspect outgoing bytes against
  the approved export, including downloads and expandable/raw representations.
  Ask pilot users who can read a share before they publish. Test stop-capture,
  retention, export, and deletion against their documented behavior. Measure
  withdrawing access and physical erasure separately; current tombstoning
  must not be reported as a purge. Peer-only encryption remains an open design
  milestone, not a satisfied KPI; define its threat model and acceptance
  checks before claiming that capability.
- **Sharing:** measure signed-in, standard-size whole-session publishing from
  final confirmation until the returned URL successfully serves the intended
  document. Keep first-share sign-in time, larger files, and future excerpt
  publishing as separate measurements. Count required actions after choosing
  material; do not count optional editing or reward skipping review. Preserve
  user cancellations as a separate outcome. A reported success with a broken
  link or the wrong document is a failure.
- **Fidelity and non-disruption:** compare output against known source content
  and explicit session identity. Missing upstream data must be labelled and
  differs from losing data that was available. Exercise crashes, partial
  writes, full disks, upstream disconnects, upgrades, and offline viewing.
  Require previously acknowledged saves to survive restart. Count an agent
  interruption caused by the recorder as a failure even if a retry succeeds.
- **Recipients:** use realistic debugging, review, and research examples.
  Agree the expected takeaway, requested action, and completion criteria with
  the sender beforehand. Start the comprehension clock when the recipient
  opens the link, including any required access step. Score correctness as
  well as speed. Asking for genuinely new evidence differs from asking the
  sender to explain material the share should already make understandable.

## Release gates and reporting

Every declared support-matrix entry and every fidelity, approved-export,
offline, and failure-recovery release check must pass. Any observed unintended
upload, wrong-session share, or corruption of source history blocks release
until resolved. Zero incidents in a finite test or pilot is an observation,
not proof that no failure is possible.

For completion rates, count successful eligible attempts divided by all
eligible attempts. Report cancellations, missing prerequisites, and unsupported
versions separately. Always include the numerator, denominator, observation
window, versions, and environment. Small pilots should show raw counts and
uncertainty; a few successful runs cannot substantiate a 99.9% reliability claim.
Use repeated benchmark runs for percentiles and report sample sizes.

Review the scorecard weekly during the pilot, run release checks before shipping,
and check upstream harness releases daily. Start by measuring the baseline and
identify the largest obstacle to a useful session view or shared understanding.
Revisit provisional targets after that baseline; record any target changes.

Usage KPIs come from explicitly opted-in pilots or consented aggregate events.
Local use must work without analytics or an account. Do not collect session
contents, prompts, paths, share URLs, or encryption keys for this scorecard.
Report the consented cohort and its limits rather than presenting it as all
users. This document does not enable telemetry or implement instrumentation.
