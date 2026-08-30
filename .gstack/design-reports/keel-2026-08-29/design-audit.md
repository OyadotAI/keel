# Keel Native UI Audit

## Classification

App UI: workspace-driven, dense, engineering-focused.

## Baseline

- Design score: C
- Visual hierarchy: C
- Typography: C
- Spacing and layout: C
- Interaction states: C
- Cross-screen consistency: D
- AI slop: B

## Verified Findings

1. Primary task, stage, and panel navigation competed without a clear hierarchy.
2. Review evidence used tiny uppercase labels and lacked a page-level reading order.
3. Extension empty states clipped at ordinary panel widths and used inconsistent typography and actions.
4. Files and History showed unexplained blank space when empty.
5. Readiness could show a zero score beside “nothing outstanding” before a scan existed.
6. Git’s icon-only remote controls were undiscoverable and its auto-commit control truncated into the history heading.
7. Conversation-to-Trace navigation appeared only on hover, hiding the relationship from keyboard and trackpad users.
8. Trace evidence did not identify the intent that produced it.

## Resolution

- Added shared content headings and sections.
- Rebuilt task tabs and stage navigation with persistent selected states.
- Added a repeatable 13-screen native visual catalog.
- Unified extension empty states around icon, orientation, explanation, and action.
- Added explicit empty/loading semantics to Files, History, and Readiness.
- Labelled Git remote actions and separated auto-commit policy from commit history.
- Made Conversation → Trace navigation persistent and anchored Trace evidence to its prompt.

## Final

- Design score: B
- Visual hierarchy: B
- Typography: B
- Spacing and layout: B
- Interaction states: B
- Cross-screen consistency: B
- AI slop: A
