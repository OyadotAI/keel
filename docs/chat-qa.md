# Chat QA — 2026-10-07

Implementation baseline: `5bf2ce1`. This follow-up fixes issues discovered during QA.

## Findings fixed

- Retrying an already completed submission now uses the daemon's authoritative running state instead of leaving the lane busy.
- A newer SSE completion wins over a delayed POST response. A confirmed submission with a lost HTTP response clears the submitted draft while retaining edits made during delivery.
- Closing a lane during workspace preparation cannot start an invisible agent turn.
- Closed stream subscriptions immediately ignore queued worker messages. Reconnection checks the selected interface after resolving the endpoint.
- Stopping a provider no longer reports its expected process exit as a runtime crash.
- Question inputs preserve digits and arrow keys when typing. Multi-select choices can be combined, and blank answers cannot be submitted.
- Codex questions with identical labels keep distinct answers through their provider IDs.
- Menus focus the first enabled action and support Up, Down, Home and End navigation.

## Verification

`make check` passed: 584 Rust tests, runtime tests, frontend tests, formatting, Clippy, lint, type checking, production build, bundle budget, sidecar staging and one Tauri test. After the final menu and handoff-test changes, `pnpm test`, `pnpm build` and `pnpm lint` passed again: 32 frontend tests and seven runtime tests in total. Lint retains 12 existing warnings and zero errors. Initial JS remains approximately 204 KB gzip against a 400 KB budget.

`python3 desktop/scripts/chat/smoke.py` passed against authenticated Claude and Codex in a temporary repository. It exercises durable submission, duplicate suppression while active and after completion, disconnected replay, two-turn native session continuity, Stop and helper cleanup. This optional test uses model tokens; normal checks do not require provider credentials.

Oya Browser (`@oya-ai/cli` and `@oya-ai/browser`) exercised the deterministic fixture at `/scripts/chat/check.html`:

- Drafts remain with their lane when switching away and back.
- Multi-select questions retain both selections; digits work in free-text answers.
- The default interface persists across page reloads.
- Enter submits, subsequent input stays in the composer, and view switches are disabled during an active turn.
- A 300-turn transcript renders formatted Markdown, code, tables and tool activity. The screenshot at 1280 × 772 shows readable wrapping and an accessible composer.

Store regression tests also cover idle chat-to-terminal handoff preserving session identity and drafts, and refusing a Codex terminal handoff when its session ID is unavailable.

## Resumed exploratory QA — 2026-10-07

Reproduced the interrupted session's UI crash in Oya Browser: opening the command palette and pressing Arrow Down emptied the React root with `TypeError: destroy is not a function`. In this browser, `scrollIntoView()` returns a Promise. The palette's expression-bodied effect returned that Promise as its cleanup. The effect now explicitly discards the scrolling result.

Oya Browser verified the fix across three cycles of arrow navigation, an empty search, searching for Settings and pressing Enter, and closing with Escape. The resumed pass also verified interface preference persistence after reload; updater installation progress, failure, Settings navigation and retry; and cancellation of the active-turn update warning. Update operations were intercepted by the native-shell fixture, and the active turn was simulated: no real installation or agent termination occurred. The narrow and desktop layouts were checked at 720px and 1280px.

The final `make check` passed outside the filesystem/process sandbox: 586 Rust tests including the Tauri test, 40 frontend tests, seven runtime tests, formatting, Clippy, TypeScript, lint, production build, and bundle budget. Lint has 12 warnings and zero errors; initial JS is 205 KB gzip against the 400 KB budget. The first sandboxed run failed three OS watcher/process-tree tests; all passed in the unrestricted run. `git diff --check` also passed.

Palette regression recipe: open the palette, press Down twice and Up once, search for a nonexistent command, press Down, search for Settings, press Enter, close Settings, then reopen the palette and press Escape. The app must remain rendered throughout.

## Release follow-up — 0.3.7

All 23 live-agent evaluations passed. Fresh Claude and Codex smoke tests passed durable submission, duplicate suppression, disconnected replay, two-turn continuity, Stop, and close. Oya also verified the minimum 720×480 window: the header, composer and status bar fit, and the projects backdrop and command palette remained usable.

Reviewing Windows CI uncovered an append-only journal handle being truncated, which Windows rejects with Access Denied. Journal loading now opens a separate writable handle only when a partial final record needs repair, then uses the append handle for subsequent writes. A regression test covers recovery, new appends, reopening, and retained submission receipts.

## Native integration limits

This is a local macOS development QA pass, not a guarantee of zero defects. Windows/Linux runtime behavior, signed/notarized installers, update delivery and every external MCP server require separate platform/integration testing. The fixture does not launch a native terminal; native PTY behavior has backend coverage but was not exhaustively exercised through the browser fixture.

Oya's desktop driver rejected `pressKey('Shift+Enter')`; typing a newline through its typing API dispatches ordinary Enter. Shift+Enter and IME guards were reviewed in the composer, but that combination was not verified by this Oya run. Oya's HTTP API also rejected read-only `run_script`; visual inspection used screenshots and page analysis instead.

The native development app was restarted after confirming its daemons had no active child agents. The separate installed Keel application was left running.
