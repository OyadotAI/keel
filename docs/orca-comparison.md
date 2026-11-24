<!-- Primary-source competitor research and the bounded Keel improvement it informed. -->
# Orca comparison — November 23, 2026

The request was to investigate Orca more deeply and improve Keel. This compares Orca's
current documentation with the current Keel checkout. Orca was researched through Oya
Browser; its application was not installed or tested. Documentation establishes advertised
behavior, not reliability, usability, adoption causes, or superiority.

## What the workflows establish

| Workflow | Orca's documented behavior | Relevant Keel foundation |
| --- | --- | --- |
| Find the next human action | A cross-worktree agents feed, blocking questions, completion updates, unread state, and links to the relevant pane. | Per-lane waiting indicators, unseen endings, errors, check results, and project/lane navigation. |
| Know what finished | Notifications describe working-to-idle transitions and structured-chat endings; experimental workers report success/failure against task and dispatch IDs. | Turn records distinguish execution from project checks; pre-PR reviews retain commit identity and stale-result handling. |
| Start from an issue | GitHub issue actions open a workspace composer with the source issue; bots delegate changes into worker worktrees and return PRs. | Manual GitHub issue preview and isolated persistent tasks, with verified fork-parent intake, frozen base commits, source snapshots, restart recovery and provider handoff. |
| Request a revision | Diff comments can be batched into agent feedback; unresolved notes can be included again. PR views can pass failed-check names and links to an agent. | Durable diff-range corrections, versioned daemon-owned batches, explicit resolution, manual GitHub check/thread refresh and live remote-head reconfirmation before dispatch. |
| Work away from the laptop | SSH workspaces, paired remote servers, experimental environment recipes, and a beta mobile companion. | A local daemon and authenticated pairing foundations; these do not establish an equivalent packaged remote/mobile workflow. |
| Run recurring work | Persistent bots accept schedules and external requests from Slack, GitHub, and Linear. | Local fixed-interval definitions using ordinary isolated tasks; one active scheduled occurrence per project, duration limits, explicit enable/pause/stop and no restart replay. External triggers and autonomous publication remain out. |

Primary sources:

- [Agents feed](https://orca.dev/docs/activity)
- [Notifications](https://orca.dev/docs/notifications)
- [Hosted reviews, issues, and Actions](https://orca.dev/docs/review/github)
- [Annotate AI Diff](https://orca.dev/docs/review/annotate-ai-diff)
- [Bots quickstart](https://orca.dev/docs/bots)
- [Orchestration](https://orca.dev/docs/cli/orchestration)
- [Ways to run](https://orca.dev/docs/ways-to-run)
- [Mobile companion](https://orca.dev/docs/mobile)
- [Supported agents and platform downloads](https://github.com/stablyai/orca#supported-agents)

Important qualifications: orchestration is experimental and its documented panel/switch is
temporarily unavailable; cloud VM recipes are experimental; mobile is beta. Bots need an
awake, reachable host. A remote server can provide that host; installing a phone application
does not move execution onto the phone.

The inspected Orca pages do not establish that every completion requires passing project
checks or a current review. This is a bounded evidence gap, not proof that Orca cannot do it.
Likewise, the comparison provides no evidence about why particular users chose Orca.

## The improvement selected for Keel

The PM selected a workspace **Attention** view: one row per lane across open projects,
ordered by pending input, errors, failed checks, and unseen endings. It should open the
correct existing surface without granting permission, retrying an answer, or executing work.

The distinction is between acknowledging an update and resolving its cause. Opening a lane
can acknowledge an unseen ending. It must not clear an unanswered request, erase a delivery
error, or hide failed checks. A completed response must remain distinct from passed checks.

This builds on Keel's evidence rather than introducing another agent runtime. It closes a
specific discoverability gap; it does not make Keel universally better than Orca.

Scope excludes global review polling, a persisted historical inbox, issue intake, bots,
scheduled agents, new remote infrastructure, mobile clients, and notification changes.
Coverage is limited to state Keel has received from open projects, with unloaded and
disconnected conversations identified honestly.

Existing Keel sources used to assess that scope include `desktop/src/activity.ts`,
`desktop/src/store.ts`, `desktop/src/ui/Sidebar.tsx`, `desktop/src/ui/Turn.tsx`,
`desktop/src/prereview/`, and `crates/keel/src/monitor.rs`.

## Implementation and verification

The implementation adds the pure Attention model, navigation/focus policy, scoped
styles, and `ui/Attention.tsx`. It integrates the entry with App, Sidebar and StatusBar, adds
request/error/Turns navigation targets, and guards Terminal readiness against taking focus
from a newer destination. A fresh shell retains its explicit composer focus request until a
new interaction supersedes it. `desktop/scripts/attention/` supplies an opt-in synthetic
fixture of the real App. Feature behavior is documented in `docs/features.md`.

Observed checks: `make check` passed on the stable `ca57ad4` snapshot plus the Attention
follow-up: 500 desktop tests, 61 runtime tests, 12 native tests, and the initial-load budget
at 346 KB gzip against 400 KB. The standalone fixture also passed TypeScript checking and
lint. Reviewer, security and reliability completed source reviews; their supported findings
were repaired and rechecked. The focused follow-up review found no supported defects.
Concurrent broad UI edits in the shared workspace were excluded from this gate and UI verdict.

Oya's connection was restored. Rendered QA covered Attention ordering, combined reasons,
source-owned request/error/check retention, update acknowledgement, exact destinations,
source resolution and removal, empty/partial/disconnected states, long lists and both themes.
It found an invalid branch API response in the synthetic fixture and a real 720×480 layout
defect that collapsed answer controls below the composer. The fixture now exercises the real
branch consumer; short windows with pending cards now use one vertical dock scroller without
unmounting the terminal. A rendered hit-target regression failed for all four answer controls
before the repair and passed afterward. Final QA reproduced that pass in both themes and
both workspace layouts with the fixture toolbar hidden.
Two stress cases also reached all 18 enabled controls across long questions, a JSON form,
a 30-step plan and a command approval. Keyboard traversal scrolled the answer field into
view; invalid JSON displayed validation, and cancelling the plan reader preserved pending
requests. Resolving that lane's last request removed its row and restored the normal layout;
resizing to 1280×800 also restored the larger layout.

Final QA verdict: **ship for the tested snapshot**, with no remaining reproduced defect in
the covered scenarios. Scratch variants were restored, the five follow-up files hash-matched
the shared checkout, the owned fixture server was stopped and the browser viewport was reset.

The follow-up preserves existing focus behavior when a request resolves. The designer's
suggestion to advance focus automatically to the next pending card was deferred by the
technical scope of this CSS repair; request lifecycle behavior was kept unchanged.

This is the real App rendered with synthetic services, not an end-to-end native agent run.
Native PTY input, OS and delayed-socket focus timing, real daemon recovery and the real folder
picker remain unverified. The concurrent live-root UI changes remain outside this verdict.

## Follow-on: supervised delivery

The subsequent request expanded the finish line beyond Attention to **issue or saved recurring
instruction → isolated persistent task → current evidence → corrections → one traceable PR**.
The original Attention scope and QA above describe that earlier increment only.

This package adds GitHub issue intake, durable correction batches, manual failed-check and
review-thread refresh, shared editable PR preparation, captured publication retries, and local
fixed-interval recurring tasks. Checks and advisory review retain separate identity and currency;
finishing a response or sending feedback cannot close a correction. A remote PR at another
commit stays unconfirmed. Pausing, editing or revoking trust during preparation fences automatic
acceptance, and an active recurring run remains recoverable outside the bounded history page.
See [Supervised delivery](delivery.md) for the complete behavior and operational limits.

External Slack/Linear triggers, autonomous publication, packaged remote/SSH execution, mobile,
calendar/cron, additional providers and licensing remain outside the expanded scope. The
advantage pursued here is a recoverable, evidence-backed supervised delivery workflow; the
comparison does not establish universal superiority or reproduce Orca's reliability.

## Adoption is a separate question

Orca advertises an MIT license. Keel's current `LICENSE.md` specifies its Solo License,
organization licensing, an unlicensed daily step limit, and usage reporting. Those are
different adoption terms; this work does not change them or infer their effect on adoption.

The current Keel source also includes Claude, Codex, Antigravity, and OpenCode integrations,
an editor, live subagent panes, and task handoffs. Comparisons based only on older Keel
architecture prose would miss these existing capabilities.
