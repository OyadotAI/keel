# Keel — working agreement

Keel is a Claude Code IDE. It drives the user's own `claude` in their repository and makes the run
visible: diffs as they are written, tool calls as one line each, a refused command as a question in
the conversation, and the project's own checks run after every turn. The audience is people who
already live in Claude Code and are tired of reading it through a terminal — so when a change is a
choice between more surface and more visibility into what the agent just did, visibility wins.

It also reads a repository and reports how ready it is for agent work and production, fixes what's
missing, and scaffolds onto Cloudflare. That half stays out of the way of the first: a repository
with no Cloudflare config is never asked about one.

**On "Cloudflare only".** That was the original line and it is no longer true. Keel reads a
Kubernetes cluster, lists GKE clusters and can create one, and shows GitHub Actions runs. What
survives of the original decision is the part that mattered: Keel does not *require* a cluster,
does not put one in the golden path, and scaffolds new projects onto Cloudflare with no Kubernetes
anywhere. The cluster surfaces are for repositories that already have one — read-mostly, and
honest about cost where they are not.

## Non-negotiables

These are enforced by tests. Changing any of them is a deliberate decision, not a refactor.

1. **The agent never gets a shell.** `--permission-mode dontAsk` + `--strict-mcp-config`; built-in
   `Bash`/`Edit`/`Write` stay denied. Every effect passes through a `keel-mcp` tool.
2. **`--bare` is never passed.** It would break subscription auth ("OAuth and keychain are never
   read"). Because of that, repo `.claude/settings.json` hooks load — so `keel-harness::trust`
   quarantines them *before* the first invocation.
3. **Deploy tools take an explicit `env`, never a default.**
4. **Dev and prod never share a stateful binding.**
5. **Promotion redeploys the proven artifact**, never rebuilds.
6. **Stop sends SIGINT**, not SIGTERM. SIGTERM abandons the turn.
7. **Listing sessions never shows what was said.** `discover_sessions` runs constantly to populate
   the switcher and returns titles, counts and timestamps only — reading a transcript to render a
   list is not licence to display it. `transcript()` is the separate, explicit path for opening one
   session the user asked for by name, and it rejects any id that could climb out of the project
   directory. Both asserted by test.
8. **A question belongs to one conversation.** `Pending` carries the `session_id` Claude Code
   already sends, and `/api/approve/poll?session=` partitions the queue rather than draining it.
   Windows are per-session now; the old `mem::take` meant whichever polled first swallowed every
   window's questions and the others timed out into a refusal nobody saw.
9. **"Allow once, this session" means that session.** `session_rules` is keyed by conversation, and
   a session-scoped rule with no conversation to belong to is refused rather than made global.
   Project rules and trust stay shared, because those are decisions about the repository.
10. **Off-loopback requires a paired device.** Loopback stays unauthenticated — the `keel approve`
    hook and the local app depend on it — but any other address demands a bearer token, and Keel
    refuses to bind beyond 127.0.0.1 at all until something is paired. The check is at the bind,
    not in the settings UI, so a hand-edited `state.json` cannot open a port either.

## The Mac app and the daemon

The application is Swift (`app/`, SwiftPM, no `.xcodeproj` — same reasoning as the bundle script:
nothing here needs Xcode's project format, and `xcodebuild` will not run until its licence is
accepted). It spawns `keel serve` as a child process and talks to it over loopback.

The split is the point. Everything that knows anything — the scanner, the workspace reader, the
permission model, the approval hook — stays in Rust and stays independently runnable as `keel scan`,
`keel workspace`, `keel serve`. Swift is the view layer. A rewrite that moved that logic into the
app would have thrown away the product in order to change the window.

**The centre of a window is the turn, not the chat.** Files changed, commands run with their output,
the gate's verdict, the duration and the cost — one reviewable artifact. All of that data already
existed; it was scattered across four panels. `make check` runs both halves (`cargo test` and
`swift test`), and the Swift side includes budgets that can fail: no `claude` left running at rest,
no more than one daemon from the app, a bundle under 40 MB, and — the one that matters — a real
launch-and-quit cycle proving **the daemon dies with the app**.

That last one is why `keel serve` takes `--exit-with-parent`. macOS has no `PR_SET_PDEATHSIG`, and
`applicationWillTerminate` runs on a ⌘Q and on nothing else: SIGTERM, a force quit and a crash all
skip it, and the daemon then reparents to init and keeps serving. The daemon polls `getppid()`
instead. The first version of the budget only counted processes *at rest*, passed happily while
every quit orphaned a daemon, and is the reason the test now launches the bundle: a budget that
cannot fail reads as proof and is worse than no budget.

The terminal is **SwiftTerm**, not a renderer of our own. Orca built its own and 678 of its issues
mention the terminal — garbled output, IME breakage in Korean and Chinese, escape sequences leaking
into the shell. The daemon's PTY framing is easy to get wrong in one specific way: **a text frame
is the tab title, not output** (`term.rs:82-83`). Treating text frames as output prints the word
`zsh` into the shell.

### The web UI is gone

`ui/` is deleted — the 6,600-line page, the 14 MB of vendored Monaco, and the `xterm` bundle. With
it went `rust-embed`, the `/` and `/vendor/*` routes, and the three handlers that existed only to
feed an editor: `api_file`, `api_original`, `api_save`, plus `read_file`, `read_original` and
`write_file` behind them. The six `ui_tests` went too; every one of them read `ui/index.html`.

It was deleted only once the Swift app could do what it did. What is deliberately *not* carried
over: file editing (there is no editor), and `/api/browse` and `/api/open-url`, which `NSOpenPanel`
and `NSWorkspace` do better natively.

## Lanes and worktrees

A lane is one conversation in the window; ⌘N gives it a checkout of its own under
`.keel/worktrees/<name>` on branch `keel/<name>`, created on the first send so the branch is named
for the ask. Every checkout-scoped request carries `?wt=<name>`, resolved by one extractor
(`serve::Checkout`); the handlers that do not take it are the point:

- **Permissions, trust and approvals read the project root.** A lane cannot carry a different
  allowlist than its repository, by construction (`AppState::checkout` is never consulted there).
- **`git branch -D` is run in exactly one place**, after the person has been shown the count of
  commits it will lose. `finish` merges with `--no-ff` and deletes with `-d`; a dirty project or a
  conflict refuses and leaves the lane untouched. Tests for each.
- `.worktreeinclude` (Claude Code's own file) lists what git leaves behind — `.env` and the like —
  and it is copied into the new checkout.
- **One dev server.** `dev.rs` is global; the preview follows whichever lane started it. Ports are
  not allocated per lane. Said in the UI rather than hidden.

Beside that: `AskUserQuestion` is in `HOOKED_TOOLS` so a question holds the turn like a command
does, and the answer travels back as a `deny` whose reason is the answer — never short-circuited
by trust, because a question is not a permission. Every turn is preceded by a snapshot
(`snapshot.rs`, a git tree from a throwaway index, no refs) so it can be rewound, and a rewind
snapshots first so it can be undone. Tokens and context size come from the stream's own `usage`
fields; Keel makes no usage API calls and shows tokens rather than a guessed percentage.

## Shipping it: never crash, know when it does, update itself

Testers crash the app on machines with no logs. Three answers, in order of how little they need:
a bar on the next launch that offers the `.ips` macOS already wrote (`Crashes.swift`); Sentry in
both halves — the app via `sentry-cocoa`, the daemon via `keel serve --sentry-dsn` — tagged
`app=keel`, `component=app|daemon`, sharing A2ABase's project; PostHog for usage counts, same
project, same tag. Both sit behind `Telemetry.swift` so the two switches in Settings › Privacy
actually silence them, and no call site ever passes a prompt, path or repository name. Keys are
read at build time by `packaging/build-app.sh` from the environment or a gitignored `.env`
(`KEEL_SENTRY_DSN`, `KEEL_POSTHOG_KEY/HOST`); PostHog falls back to A2ABase's public client key.
A build with no key has that SDK off.

Updates are Sparkle. `make sparkle-keys` once per release machine (private key in the keychain,
public key read by the build); `make release` builds, signs, notarises, writes `appcast.xml` and
publishes a GitHub release — the feed is `releases/latest/download/appcast.xml`, so nothing is
hosted. The version is the workspace version in `Cargo.toml`; bump it before `make release`.

Two rules the crashes taught: **WebKit's `takeSnapshot` returns nil for a rect outside the view
and its async import force-unwraps it** — always the completion form, always clamped to bounds
(`Preview.swift`); and **a `GeometryReader` proposes zero mid-animation** — never divide by a
size, never draw until there is room. `RenderTests` lays every pane out at 0×0, 1×1 and 2×400 so
the next one of these fails a test instead of a tester.

## Design turns and the live canvas

Click an element in the preview and the turn that follows carries a **pixel column**: the same rect
photographed before and after, with a verdict. Several clicks are several **pins**, each with its
own note, sent as one prompt and drawn on the page until the turn ends.

The other direction is the one every vibe-coding tool is criticised for missing: **when the agent
writes a frontend file, the preview comes forward, goes to that page, says what is being edited,
and ripples the regions that changed when HMR lands.** The rule that makes it work is *observe the
DOM, don't infer it*: `Picker.js` runs a `MutationObserver` armed by an `expect` message at each
write, and the mutated subtrees are both the "rebuild finished" signal and the regions to mark —
no file→element mapping, no framework knowledge. `Frontend.route(for:)` maps a page file to a URL
only when that is unambiguous (App Router, Pages Router, SvelteKit; dynamic segments are `nil`),
and is used for navigation alone. Everything the script draws carries `data-keel` so it never
reports itself as a change.

Picking elements is table stakes — Cursor ships it, and sends the agent xpath, computed styles and
fiber props. What none of them do is *check*. The cited failure everywhere is the same: the agent
guesses which source produced the element, and when it guesses wrong it edits nothing that matters
or forks a copy of the component. A green diff looks identical in both cases.

So two things are different here, and both are the same idea Keel applies to the gate:

- **The source candidates are on screen before the agent runs**, ranked, with their kind. React 19
  removed `_debugSource`, so the `data-inspector-*` attributes and the owning component name are
  the common path rather than the exception, and a ranked guess is honest where a silent one is not.
- **The pixels are re-photographed afterwards.** Identical before and after means the edit went to
  the wrong file, and Keel says so instead of letting the diff imply success. A new component file
  when the hinted source was never touched is flagged as a likely fork.

The mechanism is a `WKUserScript` with `forMainFrameOnly: false`, which crosses into the dev
server's frame regardless of origin. A page script cannot do that; a host-installed one does not
have to. This is the one feature that got *cheaper* by going native — the same thing needed a proxy
in a browser shell, and a proxy breaks HMR.

## The one hook Keel ships

`keel-harness::trust` quarantines a repository's `.claude/settings.json` and the scanner rates it
Critical, because a hook there is a shell command that runs on the machine of whoever opens the
repo. Keel then passes a `PreToolUse` hook of its own in `--settings`, and the two are not in
tension:

- Keel's hook is in the settings Keel writes and passes on the command line. It is never read from
  the working tree, so nothing a repository contains can change it.
- It points at Keel's own binary and does one thing: ask the running Keel whether the person
  approves this call, and block until they answer.
- It fails open. Keel unreachable, socket dropped, nobody at the keyboard — every path prints
  nothing and exits 0, which defers to the allowlist. A guardrail that can wedge the agent is one
  people turn off.

## Trusting a project

Per-command approval on a repository somebody already owns is not a safety property, it is a toll —
`docker`, then `wc`, then `grep`, then `sed` — and the way people pay a toll is
`--dangerously-skip-permissions`, which turns permissions off everywhere and permanently. So there
is one decision that removes the whole class: **Trust this project**.

- Scoped to one project, stored in its own `.keel/permissions.json`, and never the default. Every
  path that could set it by accident — an older store with no such field, a file that will not
  parse — reads as untrusted, and there is a test for each.
- Withdrawable, from the status bar, keeping the rules already approved. Granting something you
  cannot easily take back is a trap.
- Visible while it holds. A permission granted once and then forgotten is the one that surprises
  you later, so a trusted project says so in the status bar for as long as it is true.

The dialog names what it grants rather than asking "are you sure", which tells nobody anything.

This is what makes an approval a *question* rather than a notification. Before it, a refused
command came back as an error, the turn carried on without it, and the person's click added a rule
and asked the agent to retry — by which point it had usually worked around the gap. Verified end to
end: the call blocks, approving lets that same call proceed, denying returns a reason and the agent
stops rather than substituting.

## Layout

- `keel-scanner` — checks. Depends on nothing else in the workspace, touches no network. Keep it
  that way: it ships before any credential exists.
- `keel-harness` — `claude` supervision and trust quarantine.
- `keel-mcp` — the tool surface.
- `keel-providers` — GitHub, Cloudflare.
- `keel-generator` — golden-path templates and workload placement.
- `keel-workspace` — reads Claude Code's own state (sessions, skills, plugins, agents, commands,
  hooks, MCP servers). Read-only, and never surfaces session message bodies.
- `app/` — the Swift macOS application. A client of the daemon, and nothing else.

**Gone on purpose:** `gcp.rs` and `infra.rs` (GKE, Kubernetes, GitHub Actions runs). The cluster
surfaces were read-mostly and belonged to a different product than the one the agent loop is. What
survived of `infra.rs` is `open_url`, which now lives beside `fsops::reveal` — the other handler
whose whole job is asking the host to do something Keel deliberately will not.

## What a new project looks like

Three folders, because the halves have genuinely different constraints:

- `frontend/` — Next.js + React, compiled to a Worker by OpenNext.
- `backend/` — Hono, its own Worker.
- `infra/` — the deploy script and the environment map.

The frontend reaches the backend through a **service binding**, so the call never leaves
Cloudflare and the backend needs no public route.

**The seam is a type, not a document.** The API exports the type of its route table and the
frontend builds its client from it — no generated SDK, no schema file. That is the whole reason
both halves are TypeScript, and it is verified by deliberately asking for a field the API does not
return and checking that the frontend stops compiling.

That puts a shape requirement on the API: Hono infers the route table from one chained expression.
Assigning routes to `app` one at a time still runs, still passes the API's own tests, and silently
degrades every frontend call to `any`. It is a rule in the generated `CLAUDE.md` and a test here.

**Go was tried and dropped.** Workers run JS, TS, Python and Rust, so a Go backend needs a
Cloudflare Container or a second cloud. The container worked, but it brought manual instance
counts, ephemeral disk and cold starts on wake — and none of that buys anything a Hono Worker
does not already do for this template.

Generated projects are verified by generating one and running its own gate, not by asserting on
strings alone. Three bugs only that catches: `NextConfig` dropped `eslint` in Next 16; passing
bindings to `app.request` drops Hono's typed-response overload, so `json()` widens in tests in a
way it does not in the frontend; and, from the container version, `@cloudflare/containers` was on
0.3.x rather than the version first written. Each would have shipped a project that fails its own
first `make check`.

## The editor (gone)

There was a vendored Monaco here — 14 MB, embedded with `rust-embed`, and a long note about why
`editor/editor.main.js` could not be trimmed. All of it is deleted. Keel does not edit files: it
shows what the agent changed, as diffs you can comment on, and the comments go back to the agent.
Editing a file is what the editor you already have is for.

The one thing worth keeping from that note: **check before assuming an API exists.** That lesson
now applies to `SwiftTerm` and to WebKit's snapshot API rather than to Monaco.

## Conventions

- Rust 2024, `cargo fmt`, `clippy -D warnings`. `make check` is the gate.
- Every scanner finding must carry a `Fix`. A finding without one is a bug — it turns the report
  into a lint run nobody acts on.
- Check ids (`security/untrusted-agent-config`) are stable once shipped. Users and CI pin to them.
- Scoring is a plain total so it is predictable. Corpus tests assert exact scores; if you change
  penalties, that is a visible reviewed change.
- Comments explain *why*, especially where a platform constraint drove the design. The Cloudflare
  ceilings encoded here are the product's real asset.

## Platform facts that drive the design

- **Container disk is ephemeral** — resets to the image on every restart, `sleepAfter` 10 min
  default. Durable state never goes there.
- **D1 is single-writer at ~50 writes/sec.** Above that, Hyperdrive to a managed Postgres.
- **KV is eventually consistent**, up to 60s propagation.
- **Cloudflare has no OIDC/keyless deploy** as of Aug 2026 — scoped, rotated API tokens instead.

## Verification

`make check`. Dogfood with `make scan`, and against `../A2ABaseAI` for a repo with real CI and tests.
