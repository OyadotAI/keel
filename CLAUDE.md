# Keel — working agreement

Keel is an **ADE** — an agentic development environment. It drives the user's own `claude` in their
repository and makes the run visible: diffs as they are written, tool calls as one line each, a
refused command as a question in the conversation, and the project's own checks run after every
turn.

**Who it is for.** Product engineers. People who ship features against a real codebase every day,
who already live in Claude Code, and who are tired of reading it through a terminal. They are
fluent: they know git, they read diffs, they will notice a wrong `--no-ff`. They are not looking
for a tool that hides the machine from them — they are looking for one that shows the machine
faster than a terminal can.

Three things are the product. Everything else is in service of them:

1. **Visibility.** What the agent just did, on screen, without asking. A turn is one reviewable
   artifact: the files, the commands with their output, the gate's verdict, the duration, the cost.
2. **Git management.** A lane is a branch is a worktree is a review packet. Keel's job is to keep
   the history clean enough that a person can merge it without reconstructing what happened.
3. **Multi-agent.** Several lanes at once, each one legible on its own, none of them stepping on
   another. Concurrency that produces one confused working tree is worse than no concurrency.

It also reads a repository and reports how ready it is for agent work and production, fixes what's
missing, and scaffolds onto Cloudflare. That half stays out of the way of the first: a repository
with no Cloudflare config is never asked about one.

**On "Cloudflare only".** That was the original line and it is no longer true, and the retreat has
gone further than the note used to admit: `gcp.rs` and `infra.rs` are deleted, so there is no GKE
listing, no cluster reader and no Actions surface any more. What survives is the part that
mattered: Keel does not require a cluster, does not put one in the golden path, and scaffolds new
projects onto Cloudflare with no Kubernetes anywhere.

## The bar

The audience is picky, and their pickiness is rational: an ADE sits between them and their work all
day, so a tool that is slow, stuck or wrong about its own state costs them more than it saves. They
do not file bugs about it. They stop opening it.

So these are product requirements, not polish, and a change that trades one of them for a feature
is the wrong trade:

- **Never slow.** Every surface a person looks at more than once a minute — the diff list, the
  turn, the lane rail, the status bar — renders from data already in hand. Work that takes longer
  than a frame goes off the executor (`serve::blocking`) or off the main actor, and anything
  unbounded gets a cap before it gets a spinner. A 30,000-line lockfile is a normal thing for an
  agent to write; it must not be a normal thing for Keel to choke on.
- **Never stuck.** Every wait ends. Every request has a finite timeout (`Client.ordinary`, 90s;
  the chat stream is the one deliberate exception and it is the one thing that legitimately runs
  for minutes). Every state that can be entered can be left: a turn that ends by any path — done,
  stopped, refused by policy, cancelled mid-checkout — leaves the lane in the same state a
  finished turn does, including draining what was queued behind it. A queue that says "these run
  when this turn ends" must be emptied by *every* way a turn can end, not by the happy one.
- **Never weird.** Keel never shows a thing it cannot explain. A blank pane says which of the
  reasons it is. A command that will be refused says so before it is refused. A window that opens
  onto nothing is not a smaller bug than a crash — it is a larger one, because a crash at least
  gets reported.

The reporting in "What reports itself" exists because of this section: the failures that cost
trust are the quiet ones, so they are made loud to us and never to the person.

### Where the bar is kept, rather than remembered

Each of these was a class of bug found in more than one place, so each is now one function that
the call sites cannot get wrong individually. Adding a thirty-first `Mutex` or a fourth pipe
reader means using these, not re-deriving them.

- **`lock::Locked::locked()`** — never `.lock().expect()`. A panic in a critical section poisons
  its mutex permanently, so one panic in `approve` costs not one approval but every approval for
  the life of the process, arriving as a turn that stopped and never asked anything. `locked()`
  recovers the data, clears the poison and reports the panic. Data that is recoverably wrong beats
  a queue nobody can ever read again.
- **`git::command()`** — never `Command::new("git")`. A bare git is an interactive program that
  has not asked you anything *yet*: an askpass helper or `gpg-agent` puts up a window and waits on
  it forever, and the request behind it is a `spawn_blocking` thread that waits with it. There
  were three git helpers in three files and none of them said no.
- **`git::AUTOMATIC` + `--no-verify` on the automatic commit** — the auto-commit is Keel's
  checkpoint, not the person's commit. A repository's `pre-commit` hook is arbitrary code by the
  same author `.claude/settings.json` hooks are quarantined for, it runs on Keel's initiative
  after every turn, it has no ceiling (8.4s measured with a trivial hook; `lint-staged` is tens of
  seconds), and it re-runs the checks the gate already ran and showed. The explicit commit keeps
  its hooks, because that one is an act the person chose.
- **`lines::next()`** — never `Err(_) => continue` on `next_line()`. Undecodable bytes must be
  skipped, because stopping there leaves the pipe to fill and blocks the child mid-write; a reader
  that is actually broken must end the loop, because `continue` on an error that does not go away
  is a core at 100% inside a `tokio::spawn` nobody is watching.
- **`AppState::claim` owns "one turn per lane" and "one writer per working tree"** — both used to
  be kept in `SessionModel.start`, which is one window's array. Two `claude -p` in one lane is two
  agents on one checkout with one of them invisible to Stop, and `mark_running` simply *overwrote*
  the pid, so the first ran on with nothing left that could signal it. Two lanes writing one tree
  is the multi-agent pillar's load-bearing constraint, and tearing a lane into its own window —
  a gesture with a drag affordance and a menu item — put the pair in different arrays, so the
  check could not see the case it existed to refuse. The daemon is the only process that sees
  every window. A claim is reserved before the spawn and given back by `serve::Held` on drop,
  because a lane left claimed can take no further turn and nothing on screen would say why.
- **`Int(_: Double)` traps.** Not an exception — SIGTRAP, the whole app. Every number parsed out
  of a page (`Picked.short`) is clamped, because "the renderer always normalises that" is a
  promise about somebody else's code.
- **`git::output()`** — every synchronous git runs under a 60-second ceiling, with both pipes
  drained on threads. The drain is not tidiness: polling `try_wait` while a chatty command fills a
  64 KB pipe buffer deadlocks, and `git log` on a real repository is well past 64 KB. The ceiling
  is set against `Client.ordinary` (90s) rather than against git — a git still running at 60 will
  not produce a result anyone sees, so failing with the command named beats the window's own
  timeout with nothing in it.
- **The snapshot index is per call, not per process.** `keel-index-{pid}` was shared by every
  concurrent `snapshot()`, and one is taken at the start of every turn plus one inside every
  `restore`. Two at once meant the second `remove_file` deleted the first's index mid-`add`, and
  the first `write-tree` photographed the second's staging. It fails silently and lands later, as
  a rewind to a tree that never existed. `concurrent_snapshots_of_one_tree_agree` fails 3/3 on the
  old name.
- **Nothing unbounded reaches the app.** `MAX_DIFF_LINES` (3,000, cutting *inside* a hunk — a
  prefix's line numbers are as true as they were, and a newly written file is one hunk holding all
  of it) took a lockfile diff from 3.83 MB to 380 KB. `monitor::SHOWN` (80) sends what the UI
  reads instead of all 400 lines it keeps.
- **`serve::tests::every_handler_keeps_blocking_work_off_the_executor`** — reads the source and
  fails on any `api_*` handler that does its work inline. This is the rule with the worst failure
  mode in the file, because breaking it fails no other test: axum's pool is small, so one handler
  reading a 33 MB transcript holds up the approval poll and the chat stream, and what that looks
  like is a window that is intermittently slow for reasons nobody can reproduce. A handler that
  genuinely only touches memory says `// no-blocking: <reason>`.
- **Summaries are cached against length and mtime** (`keel-workspace::sessions`). `/api/state`
  runs on every panel and every turn end and re-read the whole project's history each time — 151
  sessions, 127 MB, 240 ms — for a list of titles. Transcripts are append-only, so length and
  mtime together identify one that has not changed. 140 ms → 18 ms warm.
- **A refusal is a prompt.** The agent is Claude Code with its own judgement, so a refusal that
  gives advice it can tell is wrong will be routed around — and the route it finds is the one that
  breaks the design. `NO_MONITOR` was one sentence for two different situations ("they said no"
  and "nobody answered in four minutes") and its advice, *run it in the foreground instead*, is
  impossible for the thing people background most: a dev server never exits, so foregrounding it
  means Claude Code's own `Bash` timeout kills it having produced nothing. Reported verbatim from
  a real turn: *"my background launch was refused. Starting it detached:"*. That is not the model
  being worse in the IDE. So a refusal now says which of the two happened, points at the dev
  server Keel already runs when that is what the command is, and closes the door explicitly —
  and `is_background` reads the *command* as well as the flag, because `nohup`, `setsid`,
  `disown` and a trailing `&` were an unguarded way past the whole of `monitor.rs`.
- **`signals::group` / `signals::end_tree`** — never `Child::start_kill()`, never a hand-written
  `libc::kill(-pid)`. Every child Keel spawns leads a process group because `claude` is not a leaf:
  a turn's real tree is `claude` with a `cargo test` under it, and a dev server is `sh` → pnpm →
  the framework → the workers that actually hold the port. Signalling the pid ends the top of that
  and leaves the rest standing, reparented to init, invisible. The negation appeared by hand in
  three places and the fourth got it wrong — the SSE disconnect path, which runs whenever a window
  or lane closes, called `start_kill()` four files away from the comment explaining why that is
  wrong. `ending_a_tree_takes_all_of_it` fails on the old form.
- **Everything that owns a process is in `watch_parent`.** It is the only cleanup Keel gets —
  macOS has no `PR_SET_PDEATHSIG` and `applicationWillTerminate` runs on ⌘Q and nothing else. The
  dev server was missing from it for its whole life, directly beneath a comment reading "a dev
  server nobody can see and nobody can stop is worse than one that never started"; measured before
  the fix, quitting Keel left it holding port 8791 with only `lsof` able to find it.
  `the_parent_death_path_stops_everything_that_owns_a_process` names the list.
- **A function nobody calls is a question, not a deletion.** A sweep for unreferenced Swift
  declarations turned up `loadCommands()`, whose own doc comment said it existed "so the picker
  works before the first turn of a session, which is exactly when somebody reaches for `/`" — and
  nothing called it. The list was written to `UserDefaults` after every turn and read back never,
  so `/` in a freshly opened project offered Keel's one own command and nothing else. Deleting it
  would have removed the evidence that the feature was broken. Check which it is first.
- **`SessionModel.closed()` for a lane leaving the window.** `stop()` deliberately leaves the
  background-job loop running, which is right when a turn ends and wrong when the lane does.
  Before this the only thing that ended it was the model being deallocated, and when SwiftUI lets
  go of a view's model is not a lifetime anyone here controls.

## Non-negotiables

These are enforced by tests. Changing any of them is a deliberate decision, not a refactor.

1. **No effect reaches the machine without a decision.** Two surfaces enforce this and they are
   not the same, so say which you mean. `keel-harness::invocation` is the locked one —
   `--permission-mode dontAsk` + `--strict-mcp-config`, built-in `Bash`/`Edit`/`Write` denied,
   every effect through a `keel-mcp` tool — and it is what `docs/guardrails.md` describes. **The
   chat panel does not use it yet** (`agent::chat`, `--permission-mode acceptEdits` or `plan`): there
   the decision is Keel's own `PreToolUse` hook plus the allowlist, which is a real gate but a
   different one. Do not write prose claiming the locked surface for the shipping path until
   `keel-mcp` has a server behind its catalog and the spawn switches to `Invocation::args()`.
   Tests: `invocation::tests::locks_the_tool_surface` for the first, `approve::tests::*` for the
   second.
2. **`--bare` is never passed.** It would break subscription auth ("OAuth and keychain are never
   read"). Because of that, repo `.claude/settings.json` hooks load — so `keel-harness::trust`
   quarantines them *before* the first invocation.
3. **Deploy tools take an explicit `env`, never a default.**
4. **Dev and prod never share a stateful binding.**
5. **Promotion redeploys the proven artifact**, never rebuilds.
6. **Stop sends SIGINT**, not SIGTERM. SIGTERM abandons the turn.
7. **Listing sessions never shows what was said.** `discover_sessions` runs constantly to populate
   the switcher and returns titles, counts and timestamps only — reading a transcript to render a
   list is not licence to display it. `tail()` is the separate, explicit path for opening one
   session the user asked for by name, and it rejects any id that could climb out of the project
   directory. Both asserted by test. (It was `transcript()` and `session_work()`, two readers that
   between them reconstructed *less* than the live decoder already produces — see "One reader".)
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
11. **One turn per lane, one writer per working tree.** `AppState::claim`, in the daemon — not a
    client. Both were kept in one window's Swift array and neither survived a second window, which
    the tear-off-a-tab gesture produces on purpose. Appended rather than inserted: the numbers
    above are pinned. Tests: `serve::tests::one_lane_takes_one_turn`,
    `one_working_tree_takes_one_writer`.

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

**What the deletion left behind is a "never weird" violation.** Nothing served `/` any more, but
the two things that *open* `/` were kept: `serve::open_ui` still opens a browser tab unless
`--no-open` is passed, and `gui.rs` — 180 lines, plus `tao`, `wry` and `muda` — still builds a
WKWebView window and points it there. So `keel serve .`, the command this file's own Contributing
note recommends, greets you with a blank 404, and `keel app` does the same in a native frame. A
window onto nothing is the exact failure the bar names, and it survived because deleting the thing
a route served is not the same edit as deleting the code that navigates to it. The fix is a
deletion, not a route.

## The conversation

The chat pane is what a person watches while a turn runs, so the two failures it can have are the
two the bar names: it can be slow, and it can show less than the terminal it replaced.

**A turn is a sequence, not two lists.** `Turn.text` was one accumulating string and `Turn.calls`
a list beside it, so the order — said something, ran a command, read the result, said something
else — was not recoverable from what was stored. `Turn.steps` is that order; `text` and `calls`
remain, because everything outside the pane (the copy buttons, the reports, the review packet)
wants the merged form. A `Step.call` references its call by id rather than holding it, so grouping,
risk and the trace pane are unchanged.

**A call keeps its whole input.** `begin` used to store one field of it — first line, 160
characters — which is a path for an `Edit` and the word `cat` for a heredoc. The arguments arrive
as `input_json_delta` and were ignored outright, so a call did not exist on screen until it was
complete; now it appears when its block opens and fills in as it is typed, which is what the CLI
does.

**A `Turn.Block` is where a delta lands.** Appending to the turn invalidated every view that read
the turn, which was both panes and every finished row in them. Appending to a block invalidates
that row. The block also holds its own parse, which is the memoisation `Markdown` never had: its
cache was keyed by the source string, and a streaming reply makes a new string per token — so every
delta missed, paid a full-string hash, and evicted a finished turn's entry on the way out. Fifty
turns meant fifty replies re-parsed from scratch per token. Nothing on the render path parses
anything now; `AttributedString(markdown:)` runs once per block, at parse time.

**Reading a value in `body` registers the dependency against that body.** `model.tailToken` was
read from an `.onChange` written inline in both panes, so every text delta rebuilt the whole
transcript. The scroll was coalesced at 80 ms; the rebuild was not. `TailFollower` is a zero-size
view that owns the dependency and calls back — the pattern to reach for whenever a pane needs to
*know* about a stream without being *rebuilt* by it.

**View `@State` outlives its model unless it is keyed.** `ChatRail` holds its scroll position as a
row id, deliberately — offsets resolve against a lazy stack's estimates. Mounted without
`.id(model.id)` the view survived a lane swap while the model did not, so the anchor named a turn
from the conversation you just left, and an id that resolves to nothing scrolls into empty space.
That is the white pane, and it is one line in `SessionWindow`.

### One reader

Claude Code appends every record to `~/.claude/projects/<key>/<id>.jsonl` as it goes — whoever
started it. The file is already a live feed, and nothing was reading it as one: `open(session:)`
read it once, from two endpoints, and stopped. A conversation running in a terminal showed a
snapshot from the moment of the click and then sat still, which reads as Keel being wrong about its
own state rather than as a missing feature.

`tail()` returns the records appended since a byte offset, and `/api/session/tail` polls it and
emits **the same `msg` events `/api/chat` emits** — because the records on disk are the shape the
live decoder already reads. So replaying a session and following one are one path, and it is the
path that has always drawn a live turn.

That was the cheap version of a feature, and the deletion is the evidence: `transcript()`,
`session_work()`, `api_session`, `api_session_work`, `SessionWork` and `moment()` all go, because a
second reader of a format is a second thing to keep in step — and this one had already drifted.
It reconstructed *less* than the decoder beside it: no reasoning, no tool arguments (they were
faked as `["command": subject]`), no raw lines, output cut at 8 KB, the call list at 300, and
`spent` records matched onto turns by index. A reopened conversation is now the one you watched.

Two rules the polling has to keep, both tested: **a partial trailing line is withheld**, because
the writer appends the object and the newline separately and a record handed out in halves is one
that parses as nothing and is never asked for again; and **the offset is a byte position**, because
transcripts are append-only so a position stays valid, while counting lines means reading all of
them to find the end. Sidechains and bookkeeping records are dropped at the same choke point the
id guard lives at, so a follower and a reader cannot disagree.

It is read-only on purpose. Two processes driving one `--resume` is a claim problem, and
`AppState::claim` is about lanes and working trees rather than conversations — so the composer says
who owns it, and typing takes it over rather than joining in.

**Reasoning does not survive a replay, and that is the format.** Claude Code writes a `thinking`
block's shape to the transcript and keeps only its signature: measured on this repository's own
session, 61 blocks, every one of them empty. The decoder reads them where they exist; nothing can
recover the ones that do not.

### The heartbeat

A turn that is thinking sends nothing at all, and the chat stream's own timeout is an hour —
deliberately, because a turn legitimately runs for minutes. So a daemon that died and an agent that
is thinking hard were indistinguishable, and the first presented as "thinking…" until somebody gave
up: a state that could be entered and not left, which is the half of "never stuck" nothing else
guarded. Every SSE stream now carries axum's keep-alive, `SSEParser` surfaces the comment line
under a name no handler can send, and two clocks come apart — `lastEventAt` (anything at all, so
sixty seconds of silence is a dead stream and fails the turn with a reason) and `lastProgressAt`
(the agent itself, which is what "quiet for 4m" has always meant).

## Lanes and worktrees

A lane is one conversation in the window. **"On its own branch"** gives it a checkout of its own
under `.keel/worktrees/<name>` on branch `keel/<name>`, created on the first send so the branch is
named for the ask. Every checkout-scoped request carries `?wt=<name>`, resolved by one extractor
(`serve::Checkout`); the handlers that do not take it are the point:

- **Permissions, trust and approvals read the project root.** A lane cannot carry a different
  allowlist than its repository, by construction (`AppState::checkout` is never consulted there).
- **`git branch -D` is run in exactly one place**, after the person has been shown what it will
  lose — commits *and* uncommitted files, because `worktree remove --force` is what makes a dirty
  checkout removable and counting commits alone let a lane with twenty unsaved files report
  nothing to lose. `finish` merges with `--no-ff` and deletes with `-d`; a dirty project or a
  conflict refuses and leaves the lane untouched. Tests for each.
- **A lane remembers where it came from**, in `branch.keel/<name>.keelbase`. Git owns that section
  — it moves it on `branch -m` and removes it on `branch -d` — so it needs no cleanup and cannot
  outlive its branch. Without it `finish` merged into whatever branch the project root happened to
  be standing on, which put a lane cut from `release` onto `main`, and `ahead` was counted against
  the wrong base, so the number a discard showed before throwing a branch away could be anything.
- **`list` asks git, not the filesystem.** `read_dir` and `git worktree list` drift apart in both
  directions: a checkout deleted outside Keel vanished from the app while git kept it registered,
  and `worktree add` then refused that name forever with a message nothing in the app could reach
  or clear. `prune` first, then parse — matching on the tail of each path, because git reports
  resolved paths and on macOS the repository's own path very often is not one.
- **Closing a lane leaves its checkout, and something says so.** The ✕ is labelled "the branch and
  its checkout stay" and for a long time nothing ever listed what stayed: measured on this
  repository, eight of them, 43 GB, three with no commits and no diff at all. `Lanes.orphans` is
  the checkouts no open lane points at, and `ProjectMenu` offers Reopen and Discard on each.
- `.worktreeinclude` (Claude Code's own file) lists what git leaves behind — `.env` and the like —
  and it is copied into the new checkout.
- **One dev server.** `dev.rs` is global and ports are not allocated per lane — a decision, not an
  accident. What *was* an accident is that its state recorded no checkout, so `status` answered
  "running", with that URL, to every lane: a lane opened the preview, saw green, and reviewed
  another lane's rendering of another lane's worktree against its own diff — and the design turn's
  pixel check then photographed an element served from the wrong tree and returned a verdict about
  it. It records its checkout now and every other lane is told whose it is.

**A shared lane is a reading lane.** `newLane(isolated:)` defaults to `false`, and "Sharing the
working tree" is offered on purpose — a lane for reading and planning beside one that is editing is
genuinely useful. For a long time it was offered and did not exist: `policy::require_isolation`
defaulted to `true` and `tighten_from` can only ever raise it, so every non-plan turn was flipped
isolated at the last moment. The offer stood in three places, the guard against two writers had
nothing left to guard, and this paragraph described something that was not there. Keel's own
default forces nothing now; only a policy file raises it. What is *not* safe is two shared lanes both **writing**, because the two things
that end a turn are tree-wide: auto-commit is `git add -A` in the checkout (`worktree::commit_one`)
and rewind restores a whole tree. Whichever finishes first sweeps the other's half-written files
into a commit labelled with the wrong prompt, and the second lane's own commit then shows less than
it did. This is the multi-agent pillar's load-bearing constraint: concurrency that produces one
confused working tree is worse than no concurrency, so a second lane that would start *writing* in
a tree another lane is writing is refused — by `AppState::claim`, in the daemon, where every window
can be seen. `SessionModel.start` still asks first and its refusal is the readable one, with the
"Give this one its own branch" button; the daemon's is the one that holds. The window covers the
tail as well (`settling`), because `running` goes false the moment the stream ends and the gate and
the auto-commit still own the tree after that.

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

`keel-harness::trust` quarantines a repository's `.claude/settings.json` and the scanner rates it
Critical, because a hook there is a shell command that runs on the machine of whoever opens the
repo. Keel then passes a `PreToolUse` hook of its own in `--settings`, and the two are not in
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

Everything below `keel` is a library with no web framework in it, and the direction of that
dependency is the point: the daemon knows about them, none of them knows about the daemon.

- `keel-scanner` — checks. Depends on nothing else in the workspace, touches no network. Keep it
  that way: it ships before any credential exists.
- `keel-generator` — the templates. `cloudflare` (the three-folder golden path), `stack` (the
  container/kustomize production shape) and `packs` (working code laid over either), plus the
  workload placement rules. Pure functions from a project name to a list of files: no HTTP, no
  tokio, no git.
- `keel-harness` — `claude` supervision and trust quarantine.
- `keel-mcp` — the tool surface.
- `keel-providers` — GitHub, Cloudflare.
- `keel-workspace` — reads Claude Code's own state (sessions, skills, plugins, agents, commands,
  hooks, MCP servers). Read-only, and the listing never surfaces session message bodies — see
  non-negotiable 7. `tail()` is the one path that reads a conversation, on an explicit ask, and it
  is also how a session running outside Keel is watched: see "One reader".
- `app/` — the Swift macOS application. A client of the daemon, and nothing else.

Inside `keel` itself, one module is one thing:

- `git` — how a git is *run*: the non-interactive environment, the 60-second ceiling, the drained
  pipes, and what a failed one says. `repo` — what Keel asks git *for*: status, diffs, branches,
  log, staging. The layering is worth keeping; the four near-identical `git()` helpers that used
  to be scattered across four files had quietly drifted apart.
- `agent` — spawning `claude` for a turn and streaming what it says. `tree` — the file tree and
  reading one file. `imports` — which files import which. These three plus `repo` were one 2,852-
  line `api.rs`, which is how a module ends up meaning nothing.
- `monitor` — background commands the daemon owns, so they outlive the turn.
- `lock`, `lines` — the two shared primitives from "Where the bar is kept".

**Where things were, and why they moved.** `keel` was 41,475 lines and more than half of it was
static template text: `packs/` alone is 22,000 lines that depend on nothing at all, sitting beside
the HTTP server and the agent supervisor, while `keel-generator` — the crate this file already
said owned the templates — was seventy-six lines. Moving them made the documentation true and cut
the daemon to 17,000 lines. The rule it leaves behind: **a template is data, and data does not
live in the crate that serves it.**

**Gone on purpose:** `gcp.rs` and `infra.rs` (GKE, Kubernetes, GitHub Actions runs). The cluster
surfaces were read-mostly and belonged to a different product than the one the agent loop is. What
survived of `infra.rs` is `open_url`, which now lives beside `fsops::reveal` — the other handler
whose whole job is asking the host to do something Keel deliberately will not.

## Two scaffolds

`keel_generator::cloudflare` lays down the golden path below. `keel_generator::stack` lays down the production
shape the team behind Keel actually runs — modelled on A2ABase: bun builds a Next.js standalone
bundle that a slim Node image runs as a non-root user, Hono on Node the same way, Postgres and
Redis from compose, nginx for the one-origin split locally, kustomize `base` + `dev`/`prod`
overlays, secrets rendered from `backend/.env` (committed only as `.env.age`), and workflows that
test, build to ghcr, decrypt, apply and roll only what changed. What the manifests insist on and
why is in the generated `k8s/README.md`; the tests in `keel_generator::stack` assert each rule. Both scaffolds
are verified the same way: generate one, install, run its gate, build it. The Next 16 `eslint`
key was caught that way, not by a string assertion.

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

## Production

The production checklist the reviewers enforce is in `docs/PRODUCTION.md`. It applies.
