<!-- Documents the supervised issue-to-PR workflow and local recurring execution boundaries. -->
# Supervised delivery

Tasks connects issue intake, an isolated coding task, evidence, corrections and an explicitly
published pull request. Task identity survives window closure, daemon restart and provider
handoff. Attention includes persisted corrections, recorded failed checks and uncertain recurring
runs, alongside live questions and approvals. A pending question takes priority over its task's
other reasons.

## Start and recover

Open **Tasks**, select the project and paste a GitHub.com issue URL. Preview the source, select
an installed provider and a local base branch, then choose **Start isolated task**. The issue must
belong to the origin repository or its GitHub-verified fork parent. A remote named `upstream`
alone establishes no relationship.

Start freezes the fetched issue and base commit, creates one worktree and registers a persistent
task. Retrying the same receipt cannot start another copy. If a response is lost, use **Check
saved start** before making another request. **Open task** recovers its current owner without
launching a provider. The original checkout must still exist; recovery does not replace it.

Use the existing Task handoff section to save progress and explicitly choose another provider.
Source, correction history, PR form and PR association remain attached to the original task.
The destination retains the checkout; old source aliases cannot become a second writer.

## Inspect separate facts

The **Delivery** tab replaces the visible Review label. It shows execution outcome, project
checks, checked commit, local commit, published commit, advisory review and unresolved
corrections separately. A response ending does not establish passed checks or mergeability.

Check evidence captures HEAD, a working-content tree and the selected check plan before and
after execution. A changed checkout, changed plan, missing capture or unavailable private proof
revokes currency. Automatic checkpoint commits can retain currency only when the known
checkpoint contains exactly the tested contents. Legacy results stay historical or unconfirmed.
These identities cover Git-visible content, not ignored files, secrets, dependency services or
the external environment.

Manual checks have durable daemon-owned receipts. Closing a tab does not cancel them. Stop
addresses the exact check receipt. Saved advisory reviews retain their commit, base, provider
and outcome; an unfinished review never becomes a confirmed verdict after restart. Review is
advisory, and publishing without it remains available.

## Correct and publish

Choose **Add correction** in a diff, click an old or new line number, and use Shift to extend a
range on that side. Keyboard-accessible fields also select the side and range. The saved
correction retains its original commit, path, range and quote. Deleted or moved locations remain
readable as historical anchors.

Save and edit task corrections in Delivery, then send the unresolved set in one chat request.
Cancelling a correction or navigating away retains its text in this window under the original task.
The daemon freezes the selected versions. A busy turn or pending permission keeps the batch
queued. Ownership, correction versions and trust changes are checked again before dispatch.
Terminal work requires an explicit transition to chat with the same provider. Sending or
finishing a batch never resolves its corrections; **Resolve** and **Reopen** are separate actions.

After publication, **Refresh PR feedback** imports failed checks and review threads by their
GitHub identities. Partial reads retain prior corrections. Sending remote feedback requires a
complete saved snapshot and a live reconfirmation of its head. Only explicit human resolution
or a complete authoritative resolved-thread observation closes a correction.
Authenticated feedback reads use the host-owned captured PR association. Project history cannot
redirect those reads; queued feedback retains that association and rejects a changed target.

The shared PR form seeds the issue link and factual verification once. Title, editable body and
draft choice are shared across Delivery and PR preparation. Local edits survive remount and
restart; **Save draft** persists them with the task. Later review reads do not overwrite them.
The body is a prepared snapshot: edit its verification section if the work changes afterward.

Publishing captures the owner, full commit, branch, base, hosting repository and form before any
push. Existing publisher safeguards and ordinary exact-ref pushes remain in force. Retries first
reconcile an existing PR; new commits update that PR. Requested and observed remote commits
remain distinct. **Saved operations → Retry captured publication** reuses the original private
request rather than silently publishing a newer checkout. Publication is always a person's action.
An existing PR at a different commit stays unconfirmed, with its observed commit and link
available. A transient hosting-read failure before capture can retry the same request.

## Recurring tasks

Under **Tasks → Recurring**, save a name, instruction, installed provider, local base, fixed
interval and run limit. Saving and editing leave the definition paused. **Enable** schedules
the next future interval; it does not run immediately. **Run now** can run a paused definition
through normal project permissions. Automatic recurrence requires explicit project trust.

Each occurrence creates a fresh isolated task and uses the same provider, ownership, checks and
delivery path as an issue task. One private project slot allows one scheduled occurrence at a
time across definitions and daemons. Overlap is recorded as skipped. Sleep, late ticks and restart
skip missed intervals; there is no catch-up queue or automatic restart of an interrupted run.

**Pause future runs** leaves current work running. **Stop active run** requests cancellation and
waits for execution, pending requests and checks to become idle. The duration includes waiting
and checks. An uncertain shutdown retains its slot. Inspect the original task and its processes
before using **Confirm original run stopped**. Elapsed time alone never releases an uncertain slot.

Execution requires the project daemon and host to be running. Revoking trust pauses future
automatic runs; trusting again does not re-enable old definitions. A replacement clone inherits
neither enablement nor execution receipts. Project relocation does not carry recurring authority;
rebind and explicitly enable definitions in the destination. Restore the original checkout when
saved task ownership cannot be recovered there.
Preparation does not grant execution authority: a successful Pause, edit or trust revocation
before acceptance prevents that automatic agent from starting. Controls that race the brief
acceptance boundary return a retryable conflict. Occurrence history is chronological and paged;
the active run and its Stop/recovery controls remain available independently of those pages.
Trust saved by older versions must be explicitly renewed in Settings before enabling automatic
recurrence. Saving ordinary command permissions preserves the current automation trust generation.

An unavailable or full execution store pauses new automatic dispatch visibly. **Retry recurring
dispatcher** checks capacity and write access, then schedules future ticks without catch-up.
Existing history is preserved. Active-run observation, duration cancellation and Stop remain
available; Retry does not release an uncertain slot. A failed startup read retains restart
quarantine until the first successful reconciliation.
Dispatcher status and Retry are shared across daemons for the same project, including when
the window is connected to a daemon that does not own the scheduler lease.

## Persistence and validation

Shared task ownership remains in `.keel/tasks/`; referenced tasks are pinned. Strict project
history lives in `.keel/delivery/state.json`. Host-owned execution receipts and schedule
authority live outside the repository, bound to its filesystem identity. A cloned project file
cannot enable a schedule or authorize a queued action. Corrupt or unsupported strict storage is
preserved and blocks execution instead of resetting to an empty runnable state. Capacity limits
reject additional writes without silently deleting unresolved work.

The automated workflow test uses a real daemon and Git worktrees with controlled packaged
runtime and GitHub adapters. It covers issue intake, duplicate launch, automatic checks and
checkpoint currency, two correction batches, one PR creation and update, publication retries,
restart, provider handoff, a recurring occurrence and tokenless management rejection.
It does not establish successful paid-provider execution or live GitHub publication. Rendered
Oya fixture QA is a separate UI check, with native-window behavior outside that fixture's scope.

Slack/Linear triggers, autonomous publication, remote/SSH execution, mobile, calendar/cron,
additional providers and licensing changes are outside this package.
