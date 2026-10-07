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

## Limits

This is a local macOS development QA pass, not a guarantee of zero defects. Windows/Linux runtime behavior, signed/notarized installers, update delivery and every external MCP server require separate platform/integration testing. The fixture does not launch a native terminal; native PTY behavior has backend coverage but was not exhaustively exercised through the browser fixture.

Oya's desktop driver rejected `pressKey('Shift+Enter')`; typing a newline through its typing API dispatches ordinary Enter. Shift+Enter and IME guards were reviewed in the composer, but that combination was not verified by this Oya run. Oya's HTTP API also rejected read-only `run_script`; visual inspection used screenshots and page analysis instead.

The native development app was restarted after confirming its daemons had no active child agents. The separate installed Keel application was left running.
