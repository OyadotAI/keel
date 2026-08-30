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

- Design score: A-
- Visual hierarchy: A-
- Typography: A-
- Spacing and layout: A-
- Interaction states: A-
- Cross-screen consistency: A-
- AI slop: A

## Premium density pass

The first resolution improved consistency but left too much simultaneous chrome. A second pass
reviewed all 13 catalog screens and reduced the interface to a calmer workspace hierarchy:

- Centered the conversation and integrated its controls into a floating composer.
- Gave assistant responses a stable visual author without boxing long prose.
- Consolidated Git's remote operations into one labelled menu and removed duplicate branch labels.
- Replaced Readiness's false pre-scan `0/100` failure state with one clear Scan action.
- Collapsed Review provenance while keeping the merge verdict and evidence immediately visible.
- Changed rail headings from tracked uppercase telemetry labels to sentence-case navigation.
- Reduced every extension empty state to one orientation sentence and one quiet action.
- Simplified commit-history copy and removed the remaining loud uppercase section treatment.

Evidence: `screenshots/catalog-premium/` contains the verified 13-screen catalog.
