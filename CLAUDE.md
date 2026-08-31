# Keel — working agreement

Keel is an ADE: an agentic development environment for product engineers, meant to replace the
terminal you run Claude Code in. It drives the person's own `claude` in their repository and makes
the run **visible** — diffs as they are written, tool calls one line each, a refused command as a
question in the conversation, the project's own checks after every turn — and it **manages the
git** around that: a worktree and a branch per conversation, a snapshot before every turn, an
auto-commit after, and a pull request at the end.

That is the whole product. Three things, done properly:

1. **Visibility.** What the agent did, while it does it, in a form you can review.
2. **Git, fully managed.** Branches, worktrees, snapshots, commits, merges, PRs — nobody has to
   be fluent in git to work this way, and nobody loses work to it.
3. **Worktrees.** Several conversations at once, each with a checkout of its own.

**Everything else has been deleted, on purpose.** The readiness scanner, the Cloudflare and
Kubernetes scaffolds, 27 project templates, the staff-engineer repo review, the AWS surface — about
35,000 lines. They were a second product wearing the first one's clothes, and the cost was not
disk: it was that the working agreement described *them*, the invariants below stopped being true,
and four bugs in a row shipped through the gap. Do not add a surface back because it would be
easy. Add it back when someone using Keel for these three things cannot work without it.


## Non-negotiables

Every one of these is asserted by a test, and every one of them is **true of the code that runs**.
That sentence used to be false: #1 said the agent had no shell and every effect went through a
`keel-mcp` tool, `keel-mcp` had no callers, and the struct whose unit tests asserted the flags was
itself dead. A guarded invariant on unreachable code is worse than no invariant — it is a promise
with a passing test and no subject. When one of these changes, change it here in the same commit.

1. **Keel drives the person's own `claude`, and `--bare` is never passed.** Bare mode never reads
   OAuth credentials, so every subscription session would fail to authenticate. The price is that
   the *repository's* own `.claude/settings.json` loads — so `keel-harness::trust` quarantines it,
   and `.mcp.json` with it, **before every invocation**, not only at project open.
2. **A refusable tool call is a question, and the turn waits for the answer.** Keel passes its own
   `PreToolUse` hook in `--settings`, pointing at its own binary; the hook asks the running Keel,
   Keel asks the person, and the call blocks until they answer. The lane's mode is `plan` or
   `acceptEdits` — never `dontAsk`, never `--dangerously-skip-permissions`.
3. **The hook fails open, and says so.** Keel unreachable, socket dropped, arguments it cannot
   parse — every path exits 0 and defers to the allowlist. It also prints one line saying that,
   because a hook that silently does nothing turns into "Keel keeps rejecting me" with no question
   anywhere and no way to tell why.
4. **The hook's command line is a shell line, so every interpolation is quoted.** Twice this has
   shipped broken. See *The one hook Keel ships*.
5. **Stop sends SIGINT**, not SIGTERM. SIGTERM abandons the turn, and the transcript is the record
   Keel exists to show.
6. **Listing sessions never shows what was said.** `discover_sessions` runs constantly to populate
   the switcher and returns titles, counts and timestamps only — reading a transcript to render a
   list is not licence to display it. `transcript()` is the separate, explicit path for opening one
   session the user asked for by name, and it rejects any id that could climb out of the project
   directory. Both asserted by test.
7. **A conversation belongs to the project it was started in.** History lists sessions from a
   shared parent directory, so two repositories under `~/Dev` are each other's neighbours.
   `session_dir_checked` returns `None` for a session started outside the open project and the
   turn is refused, naming the project — it used to fall back silently to the current repo, and
   the agent came back holding a hundred paths it could no longer read.
8. **A question belongs to one conversation.** `Pending` carries the `session_id` Claude Code
   already sends, and `/api/approve/poll?session=` partitions the queue rather than draining it.
   The old `mem::take` meant whichever window polled first swallowed every other window's
   questions, and they timed out into refusals nobody saw.
9. **"Allow once, this session" means that session.** `session_rules` is keyed by conversation, and
   a session-scoped rule with no conversation to belong to is refused rather than made global.
   Project rules and trust stay shared, because those are decisions about the repository.
10. **Permissions, trust and approvals read the project root, never a lane's checkout.** A lane
    cannot carry a different allowlist than its repository, by construction: `AppState::checkout`
    is never consulted on those paths.
11. **A background command belongs to the daemon, not to the turn.** A turn is one `claude -p` and
    the CLI kills its own background shells at teardown. See *Monitoring*.
12. **Off-loopback requires a paired device.** Loopback stays unauthenticated — the `keel approve`
    hook and the local app depend on it — but any other address demands a bearer token, and Keel
    refuses to bind beyond 127.0.0.1 at all until something is paired. The check is at the bind,
    not in the settings UI, so a hand-edited `state.json` cannot open a port either.

## The Mac app and the daemon

The application is Swift (`app/`, SwiftPM, no `.xcodeproj` — same reasoning as the bundle script:
nothing here needs Xcode's project format, and `xcodebuild` will not run until its licence is
accepted). It spawns `keel serve` as a child process and talks to it over loopback.

The split is the point. Everything that knows anything — the workspace reader, the permission
model, the approval hook, the git — stays in Rust and stays independently runnable as
`keel workspace` and `keel serve`. Swift is the view layer, and it is **drifting**: `SessionModel`
is 2,400 lines, drives 70 endpoints, and now polls background jobs, acks them and injects turns.
Each of those was a small reasonable step. Put the next one in the daemon.

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
publishes a GitHub release to the **public** `OyadotAI/keel-releases` repository (this one is
private, and Sparkle on a tester's Mac has no token) — the feed is that repo's
`releases/latest/download/appcast.xml`, so nothing is hosted. The version is the workspace version in `Cargo.toml`; bump it before `make release`.

Two rules the crashes taught: **WebKit's `takeSnapshot` returns nil for a rect outside the view
and its async import force-unwraps it** — always the completion form, always clamped to bounds
(`Preview.swift`); and **a `GeometryReader` proposes zero mid-animation** — never divide by a
size, never draw until there is room. `RenderTests` lays every pane out at 0×0, 1×1 and 2×400 so
the next one of these fails a test instead of a tester.

## Design turns and the live canvas

Click an element in the preview and the turn that follows carries a **pixel column**: the element
photographed before and after, with a verdict. Several clicks are several **pins**, each with its
own note and its own verdict, sent as one prompt and drawn on the page until the turn ends.

Aiming is the half you feel. Hover names what you would select — tag, size, best source hint — and
↑↓ walk to the parent and the first child, which is the affordance a design tool has and a picker
does not: the thing you want is nearly always the parent of the thing under the cursor. ⌥ measures
to the nearest pin. Esc disarms, and Pick stays armed until it does. ⌘-drag moves, the handles
resize, a double-click edits text — and none of that writes a file. **A nudge is a sentence**:
`width 240px → 320px` goes into the prompt in the units the source uses, the page is put back when
the turn starts so what you see afterwards is the agent's change and not your ghost of it, and the
pixel check proves the source now matches. The pop-out button gives the preview a window of its own
with `isInspectable`, so ⌥⌘I is the real Web Inspector — which is why Keel does not drive Safari:
this *is* Safari's engine, and Safari's `do JavaScript` reaches only the top frame.

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
  when the hinted source was never touched is flagged as a likely fork. The check runs *before* the
  auto-commit and holds it, because "it edited the wrong file" is a reason not to commit.

A verifier is worth exactly what its aim is worth, and three things were quietly making it report a
confident verdict about the wrong thing. **The element is located again immediately before the
after-shot** — the pick-time rect is a square of the viewport, so anything that scrolled in between
had the after photographed of whatever moved into that square. **Rects compose the frame's offset**:
each frame is told where it sits by its parent, so a pick inside the dev server's iframe is
photographed where it actually is, which is the whole case `forMainFrameOnly: false` exists for.
And **every pin is compared**, not the first of them. When no comparison is possible the verdict
says which reason — the pane was closed, the element is gone, it is off-screen — where one sentence
about the page "still moving" used to stand in for all three and was measured by nothing.

Selectors are widened until `querySelectorAll` returns exactly one node, and a selector that never
gets there is sent to the agent saying so. Tailwind's classes are kept and escaped rather than
dropped for containing a `:`; framework hash classes are the ones dropped. Svelte's
`__svelte_meta.loc` and Vue's `__vueParentComponent` are read alongside React's, so a non-React app
gets a ranking instead of nothing, and `data-testid` is offered as what it is rather than as a file.

The mechanism is a `WKUserScript` with `forMainFrameOnly: false`, which crosses into the dev
server's frame regardless of origin. A page script cannot do that; a host-installed one does not
have to. This is the one feature that got *cheaper* by going native — the same thing needed a proxy
in a browser shell, and a proxy breaks HMR.

## The one hook Keel ships

`keel-harness::trust` quarantines a repository's `.claude/settings.json` and its `.mcp.json`,
because a hook there is a shell command that runs on the machine of whoever opens the repo, and
Keel opens repositories people did not write. Keel then passes a `PreToolUse` hook of its own in `--settings`, and the two are not in
tension:

- Keel's hook is in the settings Keel writes and passes on the command line. It is never read from
  the working tree, so nothing a repository contains can change it.
- It points at Keel's own binary and does one thing: ask the running Keel whether the person
  approves this call, and block until they answer.
- It fails open. Keel unreachable, socket dropped, nobody at the keyboard — every path exits 0,
  which defers to the allowlist. A guardrail that can wedge the agent is one people turn off.

**The hook's command line is a shell line, so every interpolation into it is quoted.** Twice now
the opposite has shipped. `--lane` was added to it before the binary accepted the flag, and clap
exits 2 — which a `PreToolUse` hook reads as *block* — so every `Bash` call died. The fix made an
unparseable `approve` exit 0 instead, and that turned the next one silent: `--cwd <path>` unquoted
split on the space in `My Projects`, clap failed, the hook did nothing, and deferring means the
allowlist decides alone — every command outside it refused, no card ever queued, nobody told. It
reached us as a tester saying the agent had no access to a folder and worked in a different
project. Failing open must still *say so*, and it now prints one line to stderr.

## Monitoring

A turn is one `claude -p`, and the CLI kills every tracked background shell at teardown —
measured twice: `gh run watch` started with `run_in_background` was `[killed]` eight seconds after
the turn ended, and the person only found out six minutes on, from a `<task-notification>` that
arrived because *they* typed again. The same kill is why "run it" needed running twice. Nothing
Keel puts on the command line changes it, so the job belongs to the daemon instead:

- The `PreToolUse` hook takes any `Bash` with `run_in_background`, **before** the trust check.
  "Should this keep running after the turn" is not the question trust answered, so a trusted
  project is still asked — the same reasoning that keeps `AskUserQuestion` out of it.
- **Yes** runs it in `monitor.rs`, in the lane's checkout, outside the turn's lifetime; **no**
  sends the agent back to the foreground. Either way the agent's own call is refused, because a
  duplicate shell that is about to die helps nobody. The refusal names the job, and the system
  prompt says that naming is the confirmation.
- The completion is delivered back into that conversation as its own turn, acked first so it
  lands exactly once, and drawn as a report rather than as a blue bubble the person did not type.
- `stop_all()` on the parent-death path. `exit` alone reparents a monitored `pnpm dev` to init,
  and a dev server nobody can see or stop is worse than one that never started.

It is a list of processes with their output, not a scheduler: no cron, no retry, one run each.

## What reports itself

Testers crash on machines with no logs, and they also *fail* on them — quietly, which is worse,
because a failure nobody can see reads as the product not working rather than as a bug. So the
failure paths report themselves, and the rule is the same one the redaction has always had: the
shape travels, the content never does.

- **Every failed response, from one layer.** `report_failures` in `serve.rs` tags the matched
  route *pattern* and the status. Sixty handlers returned `(StatusCode, String)` to the app and to
  nobody else; this is one layer and covers all of them. Never the body — it quotes paths, branch
  names and the person's own text — and never the concrete path, which would carry a device id.
- **"Refused without asking."** The signature of every report that has cost trust: a tool came
  back denied and no approval card was ever shown, so there was nothing to click and no reason
  given. Reported from the app at turn end with the tool names only. It has had four different
  causes so far, which is exactly why the *symptom* is what is watched.
- **The agent exiting non-zero says why.** `classify` names the cause — `auth`, `limits`,
  `no-such-session`, `silent` and the rest — and `redact` sends the last stderr line with the
  identifying tokens removed. It used to send the exit code alone, and ten identical issues said
  "exit 1" with no way to tell a spent balance from an expired login.
- **Failing open still says so.** An `approve` invocation that cannot parse its own arguments
  exits 0, which defers to the allowlist — and now prints one line saying that commands will be
  refused without asking. Silence there is what turned an hour's diagnosis into never.

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

Four crates and an app. If a fifth is proposed, ask what it is that `keel` cannot hold.

- `keel` — the daemon: the turn (`api.rs`), the approval hook and its queue (`approve.rs`), the
  HTTP surface (`serve.rs`), lanes and worktrees (`worktree.rs`), snapshots (`snapshot.rs`), the
  gate (`verify.rs`), the terminal (`term.rs`), the dev server (`dev.rs`), background jobs
  (`monitor.rs`), permissions and trust (`permissions.rs`), pull requests (`pr.rs`).
- `keel-harness` — quarantining the repository's own agent configuration before `claude` runs.
  One file now: `invocation.rs` was deleted, because it built a command line nobody used while its
  tests asserted invariants about it.
- `keel-providers` — GitHub, for cloning and pull requests, with credentials in the keychain.
- `keel-workspace` — reads Claude Code's own state (sessions, skills, plugins, agents, commands,
  hooks, MCP servers). Read-only, and never surfaces session message bodies.
- `app/` — the Swift macOS application. A client of the daemon.

**Gone on purpose**, and not to be missed:

- `keel-scanner`, `ignored.rs`, the Readiness panel and the scan block in every system prompt —
  the repository-readiness product.
- `packs/` (27 templates, 22,000 lines), `project.rs`, `stack.rs`, `keel-generator`, `templates/`,
  `Templates.swift`, `NewProject.swift`, `Architecture.swift` — the scaffolding product.
- `review.rs` — the staff-engineer repository review. Only `# something → CLAUDE.md` survived it,
  as `memory.rs`. The **Review** tab in the window is a different thing: the current turn's own
  packet, which stays.
- `keel-mcp` — a tool surface with no callers, kept alive by a `Cargo.toml` line and a sentence in
  this file.
- `aws.rs`, `keel-providers::cloudflare`, `gcp.rs`, `infra.rs` — the cloud surfaces. What survived
  is `open_url`, beside `fsops::reveal`: the two handlers whose whole job is asking the host to do
  something Keel deliberately will not.

## The editor (gone)

There was a vendored Monaco here — 14 MB, embedded with `rust-embed`, and a long note about why
`editor/editor.main.js` could not be trimmed. All of it is deleted. Keel does not edit files: it
shows what the agent changed, as diffs you can comment on, and the comments go back to the agent.
Editing a file is what the editor you already have is for.

The one thing worth keeping from that note: **check before assuming an API exists.** That lesson
now applies to `SwiftTerm` and to WebKit's snapshot API rather than to Monaco.

## Conventions

- Rust 2024, `cargo fmt`, `clippy -D warnings`. `make check` is the gate.
- Comments explain *why*, especially where a platform constraint drove the design. That discipline
  is the reason four bugs were diagnosable from the source alone; it is the most valuable thing in
  this repository and it is not optional.
- **A deletion is a better change than an addition.** Every surface here has to earn its place
  against the three things in the first section.
- **When behaviour changes, this file changes in the same commit.** The single worst thing that has
  happened to this codebase is the working agreement describing a product that had been replaced.
- **The wire tolerates a field appearing *and* a field disappearing.** `Wire.swift` always said a
  client that fails to decode because the daemon grew a field is worse than one that ignores it;
  only half of that was implemented. Dropping `scan` from `/api/state` blanked every app built
  before the removal — the window had no project, no repository and no sessions, which reads as
  "Keel is broken" and is indistinguishable from the daemon being down. `Wire.State` now decodes by
  hand and every field has a safe absence, `project_open` included: absent reads as *open*, because
  a workbench with one empty panel is recoverable and Welcome over somebody's open project is not.
  Any response the window cannot draw without gets the same treatment.
- **One build at a time on a dev machine.** The app joins a daemon already answering on 7777 rather
  than fighting it, so a `dist` build and an installed one share whichever daemon started first —
  and before the rule above, the mismatched pair looked exactly like a bug in the app.

## Verification

`make check` — `cargo test` and `swift test`, including the app budgets and a real launch-and-quit
cycle proving the daemon dies with the app.

`make evals` before every release. Twenty-three tests that drive a real `claude` against a real
daemon: the turn, the hook, the gate, lanes, stop, rewind, monitored jobs, and the two path bugs
that shipped. They cost a few cents and a minute. `keel approve` being handed an argument it did
not accept is a fact about two files agreeing, and no unit test of either file could see it — a
build that refused every `Bash` call shipped that way, twice.
