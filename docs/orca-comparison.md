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
| Start from an issue | GitHub issue actions open a workspace composer with the source issue; bots delegate changes into worker worktrees and return PRs. | Isolated lanes, shared task memory, explicit provider handoffs, and PR preparation. Source-issue intake is a separate gap. |
| Request a revision | Diff comments can be batched into agent feedback; unresolved notes can be included again. PR views can pass failed-check names and links to an agent. | Diff inspection, project-check evidence, and the existing review feedback flow. Persistent correction tracking would require its own design. |
| Work away from the laptop | SSH workspaces, paired remote servers, experimental environment recipes, and a beta mobile companion. | A local daemon and authenticated pairing foundations; these do not establish an equivalent packaged remote/mobile workflow. |
| Run recurring work | Persistent bots accept schedules and external requests from Slack, GitHub, and Linear. | Agent sessions and supervised background commands. Background commands are not a recurring scheduler. |

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

The working-tree implementation adds the pure Attention model, navigation/focus policy, scoped
styles, and `ui/Attention.tsx`. It integrates the entry with App, Sidebar and StatusBar, adds
request/error/Turns navigation targets, and guards Terminal readiness against taking focus
from a newer destination. A fresh shell retains its explicit composer focus request until a
new interaction supersedes it. `desktop/scripts/attention/` supplies an opt-in synthetic
fixture of the real App. Feature behavior is documented in `docs/features.md`.

Observed checks: `make check` passed, including 499 desktop tests (51 new tests across the
Attention model, navigation, rendering and terminal-focus policy), the runtime and native
suites, and the initial-load budget at 346 KB gzip against 400 KB. The standalone fixture
also passed TypeScript checking. Reviewer, security and reliability completed source reviews;
their supported findings were repaired and rechecked.

UI QA remains blocked: Oya's `browser_open` and one recovery `browser_read` both returned
`Tab 26 not found`. HTTP readiness establishes that the fixture server is reachable, not that
the UI works. The QA verdict is **do not ship pending UI verification**, with no Attention
defect reproduced. Rendered navigation, keyboard behavior, delayed socket focus, narrow layouts,
screenshots, native PTY input and the real folder picker remain unverified. Restore this lane's
Oya connection through **Browser → Retry connection** and rerun QA before release.

## Adoption is a separate question

Orca advertises an MIT license. Keel's current `LICENSE.md` specifies its Solo License,
organization licensing, an unlicensed daily step limit, and usage reporting. Those are
different adoption terms; this work does not change them or infer their effect on adoption.

The current Keel source also includes Claude, Codex, Antigravity, and OpenCode integrations,
an editor, live subagent panes, and task handoffs. Comparisons based only on older Keel
architecture prose would miss these existing capabilities.
