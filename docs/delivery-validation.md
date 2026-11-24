<!-- Records the implemented scope, observed validation and remaining release checks. -->
# Delivery implementation status

The request was to investigate Orca and improve Keel. The implemented package connects
GitHub issue intake or a saved recurring instruction to an isolated persistent task, current
check evidence, durable corrections and an explicitly published or updated PR.

PM scoped in that supervised workflow and local fixed intervals. Remote/mobile operation,
Slack/Linear triggers, autonomous publishing, calendar/cron, additional providers and licensing
changes remain outside this package. Designer selected existing workspace surfaces with separate
execution/check/review/publication facts. Principal selected existing chat, worktree and publisher
facades, private execution authority and exact checkout evidence. No stage was overruled.

Changed domains: `crates/keel/src/delivery/`, `execution/`, `github_read/`, task recovery,
chat/check/review/publication integration, `desktop/src/delivery/`, Tasks/Delivery/diff UI,
Attention, controlled workflow fixtures and documentation. No new dependency was added.
See [workflow documentation](delivery.md) and [comparison](orca-comparison.md).

Reviewer, security and reliability approved their reviewed repairs. Observed integrated
`make check` passed with 820 Keel unit tests, 24 daemon evals, 532 desktop tests, 61 runtime
tests and 16 native tests; one existing live-browser test was ignored. Initial bundle was
357 KB gzip against 400 KB. This gate preceded the final three rendered-QA repairs.

The real-daemon controlled test used scratch Git worktrees and controlled runtime/GitHub
adapters. It covered duplicate launches, current checks/checkpoint identity, two correction
batches, one PR create/update with retries, restart, provider handoff and recurrence. It did
not execute a paid provider or publish to live GitHub.

Oya QA verified issue preview/start and owner recovery, original diff range/SHA retention,
cancelled drafts, unresolved correction delivery versus explicit resolution, shared PR drafts,
persisted Attention recovery, recurring controls, dispatcher retry and old active-run recovery.
Its first verdict was **do not ship yet**, for three P2s: persisted Attention ordering, a stale
publication label after successful retry, and the shell covering Delivery at 720×480 in Coding
Mode. All three were repaired. Twenty-five focused desktop tests, TypeScript and targeted lint
passed. A subsequent Oya hit-target check passed for Delivery controls at 720×480 in light
Coding Mode. The final specialist QA rerun was not performed after the user requested wrap-up.

Latest gate after those repairs: all 537 desktop assertions passed, but Vitest reported two
unhandled asynchronous teardown errors from `src/relocate.test.ts` (`refreshJobs` and
`refreshDev`), so `make check` failed at desktop tests. This is not a green final gate.
The earlier complete gate remains the latest full pass. The concurrent `ai.oya.factory`
namespace edit was preserved; its packaging assertion was aligned and all packaging tests passed.

Work stopped at the user's wrap-up request. Changes are integrated in the main working tree;
they have not been released. The owned fixture servers were stopped and Oya's viewport reset.

Remaining release verification: rerun the three repaired UI scenarios in both themes/layouts.
Native PTY workflows, literal bracket keydown through Oya automation, live GitHub, paid providers,
physical disk-full behavior and host shutdown/orphan-child behavior remain unverified.
This report establishes the implementation and bounded checks, not universal superiority.
