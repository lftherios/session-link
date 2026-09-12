# What makes session.link succeed

2026-09-12 · product direction from the planning conversation.

These are product requirements and evaluation criteria, not claims that every
part is already delivered. Use them with the [product plan](product-plan.md) to
prioritize work. Improvements should make the core experience easier, faster,
more dependable, or more trustworthy.

## The nine agreed criteria

1. **Installing anywhere is easy and fast.** One obvious command should take
   someone from a fresh machine to their first useful session view. Support
   the places agents actually run: laptops, remote machines, containers, and
   development environments across the supported operating systems and
   architectures. Upgrading and uninstalling should be just as straightforward.
   Track installation success, manual steps, download size, and time to the
   first useful view on clean environments.

2. **The daemon is tiny and extremely efficient.** Background recording should
   be cheap enough to leave running without thinking about it. Aim for
   negligible idle work, bounded memory, controlled disk growth, and minimal
   added latency under load. Measure binary size, startup time, resident memory,
   idle CPU and wakeups, active resource use, and streaming overhead. Existing
   harness history must remain viewable without enabling the recorder.

3. **Harness integrations stay current and work flawlessly.** Compatibility
   is a continuing product responsibility. Follow upstream releases, test
   supported versions through real handoffs as well as fixtures, and treat
   integration regressions as core product bugs. Cover discovery, explicit
   session identity, resumed and concurrent work, tool results, and native
   invocation. Publish an accurate capability matrix and report unsupported
   or changed formats clearly. Adding a logo does not establish an integration.

4. **The viewer is beautiful and inviting to use.** Reading, searching,
   exploring activity, selecting a passage, and preparing a share should feel
   coherent and responsive. Preserve the reader's place and make large sessions
   comfortable on real devices. Keyboard access, readable typography, and
   narrow-screen layouts are part of that quality. Evaluate actual interaction
   and repeat use alongside load time and interaction latency.

5. **Users always control their data.** Local use comes first. People choose
   what is recorded, retained, included in a share, and published, and understand
   who can read it. Make stopping capture, exporting, changing retention, and
   deleting data understandable. Local files and the open format should remain
   useful independently of the hosted service. Prototype encrypted sharing
   using iroh-blobs, with locally tested recovery and device approval; access for named recipients remains future work.

6. **Sharing is super simple.** Move from the intended session or passage to
   a useful link with minimal steps. A whole-session share should be easy;
   selection and an author note should be available when helpful. Keep the
   chosen material stable through sign-in, retries, and continued agent work.
   Return and copy the resulting URL clearly. Measure time, decisions, and
   recovery steps from intent to a usable link.

7. **The record is trustworthy.** Open and share the session the user intended.
   Preserve available prompts, outputs, tool results, relationships, and source
   provenance. Distinguish captured content from author notes and reconstructed
   history. Make missing or incomplete evidence explicit; partial capture must
   not appear complete. Validate concurrent sessions and interrupted writes.

8. **Session.link never gets in the way of agent work.** A stopped recorder,
   full disk, network failure, or upgrade should not corrupt the original
   history or unnecessarily stop the agent. Define and verify failure and
   recovery behavior, including a clear indication when capture is incomplete.
   Local reading should remain useful offline.

9. **The recipient gets value immediately.** A colleague should understand
   why the session was shared, where to begin, and how to inspect the evidence.
   They should not need the originating harness or a tutorial. Keep any access
   step appropriate to the sender's chosen audience. Verify this with real
   exchanges: can the recipient answer the question or take the next action
   without asking the sender to reconstruct the story?

## Encrypted-sharing prototype and remaining decisions

Local-first use is an agreed requirement. The selected prototype direction is
client encryption with iroh-blobs, a persistent hosted provider and ordinary
browser access. The service should hold ciphertext without content keys.
See [identity and encryption](identity-encryption.md) for the accepted scope
and proposed sign-in model. This is not a current capability or production
dependency decision. Resolve recipient key verification, forwarded links,
key recovery, device changes, and what revocation or deletion can promise
after someone has received a copy. Direct peer-to-peer delivery and
end-to-end encryption are separate design decisions.

The current hosted flow uses unlisted links readable by anyone who has the
link. The server can read uploaded content; local captures are plaintext.
Current hosted deletion hides the published share while retaining its blob.
Do not describe those behaviors as peer-only encryption or complete erasure.
A private sharing design must account for the current server's validation,
secret scanning, rendering, and link previews without exposing private content.

## How to judge progress

The [KPI scorecard](kpis.md) proposes numerical targets, measurement definitions,
and release gates for all nine criteria. Targets are provisional and are not
measured results. Establish baselines with representative clean installs,
realistic large sessions, supported harness versions, and actual
sender/recipient exchanges before adopting performance commitments.

The proposed product outcome is **repeat useful handoffs**: people return to
session.link because it helps them understand their own agent work and helps
colleagues act on it. Measure installation and sharing friction, compatibility
failures, resource overhead, recipient understanding, and repeat use without
collecting session contents by default.
