# Keel Native UI Audit

## Classification

App UI: workspace-driven, dense, engineering-focused.

## What the earlier grades measured

An earlier pass in this file recorded a move from C to A− across visual hierarchy, typography,
spacing and cross-screen consistency, and a "premium density pass" on top of it. The catalog it
cited as evidence — `screenshots/catalog-premium/` — did not support that, and several fixes it
listed as resolved were visible as unresolved in the very screenshots it pointed at.

The reason is worth writing down, because it is the failure mode of any design audit:

> **It measured whether shared components were created, not whether they were adopted.**

They were created. They were largely not adopted.

- `ContentHeading` and `ContentSection`, added as "the heading rhythm used by primary content
  surfaces", had **0 and 1** external uses.
- `Blank`, the good empty state, was private to five extension panels. The five core workspace
  panels each rolled their own: **four empty-state idioms and about ten bare `Text`s**.
- `hint()` exists so an icon-only control is readable by VoiceOver. **57 controls still used raw
  `.help()`**; 15 used `hint()`. The app had five `accessibilityLabel`s in total.
- "The one filled button" was **three** near-identical styles, two of them differing by a single
  point of vertical padding, each documented as the only one.
- `QuietButton` — the most-used control in the app, 74 call sites across 24 files — was defined in
  `TurnStage.swift`, a view file.
- **147** `.font(.system(size:))` calls bypassed the type scale. **115 were `size: 10`**, a size
  the scale had no name for.
- One eyebrow treatment was copy-pasted **22 times across 13 files**, with three different tracking
  values for a single intent.

## What was done

Every fix below is either *adopt the component that already exists* or *name the token whose
absence caused the drift*. Very little was invented.

**One home.** `QuietButton` and `asButton` moved into `Theme.swift`. `SendButton` and
`SendButtonWide` were deleted into `FilledButton`, which gained the disabled appearance that was
the only reason they existed.

**One type scale.** `K.F.tiny`, `K.F.codeTiny` and `K.F.reading` name the three sizes the app was
already using most and had no words for. Nothing outside `Theme.swift` builds a font now.
`.sectionLabel()` replaces the 22 eyebrows, with one tracking value.

**One spacing scale.** `K.S` grew `hair`, `tight` and `snug`, because 1, 3 and 5 were already
written out roughly 120 times — including inside `HoverRow`, `FilledButton`, `QuietButton` and
`Pill`, the components that define the grid.

**One empty, loading and error state.** `EmptyState`, `Loading` and `ErrorRow` replace four, five
and four idioms respectively. Every field but the message is optional, which spans the whole range
from a full icon/title/body/action panel state to a single line of explanation. The inset matches
`RailHeader`'s, so the first thing in a panel lines up with its title.

**The states that did not exist.** A failed refresh now reads as failure rather than as an empty
panel; Files distinguishes an empty tree from one that has not arrived; Readiness says when the
scan did not run. And `lastError`, which four surfaces rendered at three type sizes so one git
failure could appear three times at once, appears once and can be dismissed.

**Navigation that describes where you are.** The stage bar offered three tabs for seven surfaces:
opening a diff, a file, a commit or an inspector left no tab lit and no way back but an ✕. The tab
you came from stays selected and a breadcrumb names where you have gone, with ⌘[ to return. The
stage no longer throws away what you were reading when a turn starts.

**⌘K reaches the app.** It covered about a fifth of it, and was asymmetric on what it did cover —
Approve had rows, Deny had none. All ten panels, all three stages, the git verbs, past sessions,
Deny, and "show me this diff" are now typeable.

## Two bugs and an accessibility gap found while auditing

- Every `model.sheet` sheet was anchored on `SidePanel`, which leaves the hierarchy when the panel
  is collapsed. **⌘K → "Project setup" with the panel closed did nothing at all.**
- The terminal toggle was on screen twice at once, toolbar and status bar, same glyph.
- Three controls existed only on hover — `opacity(0)` or `if hovering` — which is exactly what
  `FileDiff`'s own comment forbids: *"A control that only exists on hover does not exist for the
  keyboard."*

## What keeps it

Four source-scanning tests in `app/Tests/KeelAppTests/ThemeTests.swift`, alongside the type-floor
test that was already there:

- nothing outside `Theme.swift` builds its own font or uses a stock text style
- nothing outside `Theme.swift` defines a button style
- padding and spacing come off the `K.S` scale
- an icon-only control uses `hint()`, not `.help()`

Each was verified to fail when the rule is broken. The type-floor test that already existed did not
prevent any of this, because it only asked whether a literal was too *small* — never whether it went
through the scale at all. That is the difference between a floor and a system.

## Evidence

`screenshots/catalog-unified/` is the current 13-screen catalog, regenerated from
`VisualCatalogTests`. `catalog-before/`, `catalog-after/` and `catalog-premium/` are the earlier
passes, kept so the claim above can be checked rather than taken.

## Not addressed

Named rather than left to rot: the tabbed terminal with ⌘T and per-tab process names described in
`docs/features.md` does not exist; Problems rows are advertised as clickable and are plain text;
drag-and-drop accepts only `.fileURL`, so an image dragged from a browser is silently ignored; and
commenting on a diff line — the product's headline verb — is a bare tap gesture with no gutter cue,
tooltip, context menu or keyboard path. Each is its own piece of work or a documentation fix.
