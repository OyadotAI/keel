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
  its hooks, because that one is an act the person chose. `AUTOMATIC` also points
  `core.hooksPath` at nothing: `--no-verify` skips two hooks, and `post-commit` still ran.
- **`writes::`** — every file Keel writes into a project on the person's behalf (adopt, a
  subagent, a skill) goes through it. `hold` takes the tree's writer claim like a turn, because
  adopting mid-turn committed the whole team under that turn's prompt. `create_new` and
  `no_link_under` never write over a file, through a link, or into another repository — a
  dangling `pm.md` link once had adopt create a file outside the repo. `commit_only` commits its
  own paths and nothing of the person's: not during a merge, not an ignored path, not an
  instructions file whose content (not `git diff`, which trusts `assume-unchanged`) differs from
  HEAD, and it backs out what `add` staged when anything fails.
- **`serve::tests::every_handler_keeps_blocking_work_off_the_executor` reads every module from
  disk.** A hand-kept list of ten missed thirteen modules; before that the test stopped reading
  at the first `#[cfg(test)]` attribute and checked a third of `serve.rs`.
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
- **`serve::tests::every_handler_keeps_blocking_work_off_the_executor`** — reads the source of
  ten modules and fails on any handler that does its work inline. This is the rule with the worst
  failure mode in the daemon, because breaking it fails no other test: axum's pool is small, so
  one handler reading a 33 MB transcript holds up every other request, and what that looks like
  is a window that is intermittently slow for reasons nobody can reproduce. It was kept for
  `serve.rs` alone for a long time, and `dev::status` walked a checkout's `package.json`s on the
  executor for its whole life. A handler that genuinely only touches memory says
  `// no-blocking: <reason>`.
- **`turns::emit` is the one path a fact takes** — into `.keel/turns/<session>.json` under one
  lock, atomically, then onto the per-session bus. A fact written and not broadcast is a replay
  that disagrees with the live view; one broadcast and not written is the reverse. The store
  carries its own `.gitignore`: the follower's first test found git attributing
  `.keel/turns/<session>.json` to the turn and the checkpoint about to commit Keel's records into
  the project.
- **`events::after_mutation` says what a request changed, from the path it took** — one
  middleware rather than a call in sixty handlers. The daemon's own hands emit where they change
  things nobody asked for (a job's output, a dev server's URL, the checkpoint after a turn), and
  two watchers cover what happens outside Keel. `BudgetTests.testTheMainPageDoesNotPoll` pins
  every `Task.sleep` left on the main page by name, so the list can only shrink.
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
    the tear-off-a-tab gesture produces on purpose. A terminal `claude` is a writer too: a busy
    one holds `term:<session>` on its tree — through its follower when a window has it open, and
    through the sessions watcher (`events::hold_terminal_claims`) when none does — so a lane
    starting to write that tree is refused, and a terminal turn opening in a lane's tree gets its
    files noted and nothing checked or committed. Appended rather than inserted: the numbers
    above are pinned. Tests: `serve::tests::one_lane_takes_one_turn`,
    `one_working_tree_takes_one_writer`, `turns::tests::a_terminal_writer_refuses_a_keel_lane_on_the_same_tree`,
    `events::tests::an_unfollowed_busy_terminal_session_holds_its_tree`.

## The desktop app and the daemon

The application is `desktop/`: Tauri 2, a React page, and one `keel serve` per open project. It
replaced the Swift macOS app (`app/`, deleted 2026-10-05) because testers reported hangs while a
reply streamed and on every lane switch, a chat pane that went blank, sessions cut off by sleep,
no way to open two projects, and no Windows build.

The split is the point, and the rewrite moved knowledge *into* Rust rather than out of it: the
scanner, the workspace reader, the permission model, the approval hook, the turn — and now the
conversation decoder (`keel-workspace::conversation`) — are Rust, and stay runnable as `keel scan`,
`keel workspace`, `keel serve`. The page draws ops; it decodes nothing a second client would have
to decode again.

**One window, a sidebar of projects, lanes under each.** `desktop/src-tauri/src/main.rs` is the
whole native side: it starts a daemon per project on a free port the first time the project is
used, ties each one to itself, and hands the page `{port, token}`. One daemon per project rather
than one for all: a daemon's state — claims, the dev server, the watchers — is one project's, and
several side by side change nothing inside any of them.

**The daemon dies with the app through its stdin.** `--exit-on-stdin-eof`: the app holds the
write end of a pipe and the kernel closes it however the app dies — ⌘Q, a crash, a force quit,
on Windows too, which has no parent to poll. The Swift app's `--exit-with-parent` (`getppid`)
is still accepted. `tests/lifecycle.rs` is the budget that cannot be faked: it starts the binary,
lets go of the pipe and fails if the daemon is still there ten seconds later.

**The page is a page, and the daemon knows its own.** The webview's origin (`tauri://localhost`,
`http://tauri.localhost`) is shared by every Tauri app on the machine, so the origin alone proves
nothing: the app writes a random token as the first line of the daemon's stdin, and
`pair::app_token` lets a page in only with that token beside an `APP_ORIGINS` origin — in
`Authorization`, or as a `keel.<token>` WebSocket subprotocol for the terminal, since a page's
WebSocket cannot set headers. Loopback without an `Origin` (the hook, the CLI) is unchanged.

**What keeps it fast, each a rule in the code rather than a habit:**
- SSE is read and JSON-decoded in a Web Worker (`stream.worker.ts`) and handed over in one batch
  per 16 ms — one store update per frame however fast the agent writes.
- `reduce.ts` copies the turn an op touches and the one block or call inside it; a view
  subscribed to a block (`Turn.tsx`) re-renders when that block changes and never for a sibling.
- The transcript and every long list are `react-virtuoso`; the chat follows the bottom only
  while you are at the bottom, and nothing computes a scroll target from estimated heights.
- `Conversation` is keyed by lane, so nothing in it outlives the lane it was drawn for — the
  Swift pane's scroll anchor named a row from the lane you had left and scrolled into nothing.
- Diffs are prepared once on arrival (`git.ts`), never per render.
- `pnpm budget` fails the build past 400 KB gzip at launch (150 KB now); the terminal is a lazy
  chunk.
- A long action started from a panel — a pull request, a check, a plugin install — lives in
  `runs.ts`, not the panel's state. A panel unmounts on a tab switch; a run held there came back
  as an idle button over a `gh pr create` still pushing, and a second click made a second PR.
- One of each primitive (`ui/kit.tsx`: `Tabs` with roving focus, `Empty`, `Banner`;
  `InlineDiff`), after Oya's browser renderer. Four tablists had been hand-written and none
  took an arrow key. Tokens are Oya's too — shadows with a hairline instead of 1px borders, one
  focus ring, one spring — and DM Sans ships in `src/fonts`.
- `pnpm lint` is in the gate: typescript-eslint, the rules of hooks, and size budgets as a
  ratchet (`--max-warnings`): the count of oversized functions can go down and never up.

`make check` runs `cargo test` and `desktop-test` (type-check, vitest, bundle, budget).
`make dev` runs the app against a daemon built from this tree. The terminal is xterm.js, for
the reason the Swift app used SwiftTerm: Orca built its own renderer and 678 of its issues mention
the terminal. The PTY framing rule stands — **a text frame is the tab title, not output**.

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

**The decoder is in Rust, and every ending closes every call.** Claude Code's stream-json (and
Codex's events) become `turn` ops in `keel-workspace::conversation`: `open`, `text` appends,
`call` upserts, `result`, `usage`, `failure`, `raw`, `close`. `/api/chat` and `/api/session/tail`
send them when asked with `?ops=1`, and the raw `msg` records otherwise (the evals read those).
Three properties, each tested there: `close` answers every call still running with
`result{interrupted}` first — `agent::translate` is the one place a chat turn closes, before
`done`, before `fatal`, or when the task drops its sender — so no row is left spinning; a step's
id is `<message id>:<block index>` on both paths, so live and replay fold to the same turn; and
text goes out as appends batched at 40 ms, so no byte is sent twice. The Swift decoder this
replaced was 4,300 lines and had all three bugs.

**A call keeps its whole input.** It appears when its block opens, its arguments are buffered
(capped at 2 MB) and arrive when they parse, and the complete message repairs them. Output is
capped at 64 KB, head and tail.

### One reader

Claude Code appends every record to `~/.claude/projects/<key>/<id>.jsonl` as it goes — whoever
started it. The file is already a live feed, and nothing was reading it as one: `open(session:)`
read it once, from two endpoints, and stopped. A conversation running in a terminal showed a
snapshot from the moment of the click and then sat still, which reads as Keel being wrong about its
own state rather than as a missing feature.

`tail()` returns the records appended since a byte offset, and `/api/session/tail` is woken by
a file watch on the transcript — a 2 s sleep is the fallback, not the mechanism — and emits **the same
`msg` events `/api/chat` emits**, because the records on disk are the shape the live decoder
already reads. So replaying a session and following one are one path, and it is the path that
has always drawn a live turn. Both streams carry `fact` events as well; see "Facts". The first
read of a transcript is its tail — at most 8 MB, on a record boundary — and no read takes more
than that in one go; a 33 MB session was three copies of 33 MB on the executor's behalf before.

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

### Facts: what the daemon knows about a turn

Claude Code's transcript records what was said and which tools ran, and that is all it owes
anyone. Everything else on a turn — the tree before it ran, the files git says it moved, the
gate's verdict with its problems, the commit, what it cost, what it asked — is Keel's own reading
of the machine, and for a long time it was computed in the app at the end of a turn the app
owned and kept nowhere, so a relaunch rebuilt a turn with its prose and none of its work, and a
followed turn never had any. Now every one of those is a **fact** (`turns.rs`): the daemon writes
it to `.keel/turns/<session>.json` and broadcasts it on a per-session bus, and both streams carry
it — live as it happens, on replay interleaved right after the record that opened the turn, so
a reopened conversation shows exactly what the live view showed. The app consumes them in one
place (`SessionModel.fact`) and computes nothing of its own at the end of a turn any more; the
one fact it still posts is the pixel verdict, which only it can take, addressed by lane.

Three things about the shape are load-bearing. **A record is keyed by the `uuid` of the human
`user` record that opened the turn**, not by an ordinal: the daemon drops the head of a long
replay, a compaction summary looks like a prompt, and any change to what a follower is shown
renumbers everything after it; the uuid is stable because transcripts are append-only. **A
Keel-driven turn is keyed off its own transcript** — the daemon notes where the file ended before
spawning and reads the first prompt record after that — and facts that arrive before the key is
known travel keyless, which on a chat stream can only mean the turn the lane has open. **`finish`
is the lifecycle, for every turn whoever drove it:** files against the fingerprint `begin` took,
the gate with `running` said first, the design verdict waited for when one is coming, the commit
if the gate did not say no and the tree is still this turn's, the cost, the end — under the
lane's claim for a lane, and under `term:<session>` for a terminal session, whose turns the
`Follower` closes on Claude Code's own turn-end note or its pid file saying idle, and then quiet.

### Events: what changed

The app used to find out by asking — the session list every three seconds while History was
open, the jobs every two or fifteen per lane for the life of the window, approvals every 700 ms
for the length of every turn, git after every turn and every time the app came to the front.
Four idle lanes asked the daemon for the same empty job list 172,800 times a day and still found
out late. `GET /api/events` (`events.rs`) says what changed, one subscription per window
(`DaemonEvents`, fanned out; `Lanes.route` is the one door). The events are coarse on purpose —
"git changed, read it again" rather than a patch — except the small ones, which carry their
payload. A connection that drops comes back on its own and says so: the status bar reads
RECONNECTING for as long as it is true, where before a dead daemon read as every panel quietly
emptying. A `connected` frame on every reconnect is what tells every store to read again,
because whatever happened in the gap is gone.

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

**Updates are the Tauri updater** against `latest.json` on the public `OyadotAI/keel-releases`
repository (this one is private, and an app on a tester's machine has no token). It checks at
launch and every six hours, downloads in the background, and installs only on **Restart to
update** — a restart ends every agent the app runs, so the status bar asks first when a lane is
mid-turn, and that is never Keel's call. `make release` bumps the workspace and the shell
(`desktop/src-tauri/Cargo.toml`) together and pushes the tag; `release.yml` builds the universal
macOS app and the Windows installer, signs, notarises, staples the DMG and publishes. The daemon
ships inside the app (`externalBin`, staged by `packaging/sidecar.sh`). Details and secrets:
`packaging/README.md`.

**Installed Swift builds reach this one through Sparkle.** The Tauri app keeps the Swift app's
bundle identifier (`ai.oya.keel`) and Team ID, so the old updater accepts it as the next version
and installs it in place; the release still writes a Sparkle `appcast.xml` for them. Changing the
identifier strands every one of those installs on the last Swift build.

**Crash and usage reporting is not ported yet.** The Swift app had a crash-report bar, Sentry and
PostHog behind `Telemetry.swift`; the desktop app has none of them, and the daemon's
`--sentry-dsn` is not passed. Do not describe reporting as present until it is.

Two rules the crashes taught: **WebKit's `takeSnapshot` returns nil for a rect outside the view
and its async import force-unwraps it** — always the completion form, always clamped to bounds
(`Preview.swift`); and **a `GeometryReader` proposes zero mid-animation** — never divide by a
size, never draw until there is room. `RenderTests` lays every pane out at 0×0, 1×1 and 2×400 so
the next one of these fails a test instead of a tester.

## Design turns and the live canvas

**What the desktop app has, and how it differs.** The Preview tab is a child webview
(`desktop/src-tauri/src/preview.rs`) laid over the panel, with `picker.js` installed in every
frame — the Tauri form of `forMainFrameOnly: false`. Pick, hover names the element and its best
source, ↑↓ walk parent and child, Esc disarms, and each pin takes a note; **Send to agent** types
one prompt into the lane's CLI. When the lane goes idle the pins are located again and compared.
Three differences from the Swift app below, each deliberate:

- **The verdict compares a fingerprint, not pixels**: the element's markup, its computed style
  and its size, hashed. Tauri has no snapshot API on either platform, and "identical before and
  after means the edit went to the wrong file" is the same answer from the DOM — on macOS and
  Windows alike. What it cannot see is a change only a canvas or an image would show.
- **The dev server's page may call exactly one command.** It runs the project's dependencies, so
  `build.rs` names every app command (so none is reachable by default) and
  `capabilities/preview.json` grants the preview `preview_msg` and nothing else.
- **A native webview draws above the page**, so menus, dialogs and the palette count themselves
  in `cover.ts` and the preview hides while any is open — otherwise they open behind it.

Not ported: nudges (⌘-drag, resize handles, text edit as a sentence), ⌥-measure, and the preview
coming forward and rippling the regions HMR changed. `pnpm picker-check` runs the picker in a real
browser (Chrome) against ten behaviours.

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

- The `PreToolUse` hook takes any `Bash` with `run_in_background`, **before** the trust check,
  and runs it in `monitor.rs`, in the lane's checkout, outside the turn's lifetime. The agent's
  own call is refused, because a duplicate shell that is about to die helps nobody; the refusal
  names the job, and the system prompt says that naming is the confirmation.
- **Nobody is asked.** It was a card for a while, and the card was the wrong shape: "should this
  keep running after the turn" is not a permission — `Bash` may already be allowed and the
  project may be trusted — and there was no second answer worth having. *No* hands a dev server
  back to a foreground `Bash` timeout that kills it having produced nothing, and a card nobody was
  at the keyboard for cost four minutes before arriving at the same place. Monitors is where it is
  answered for instead: listed while it runs, with its output and a Stop button. **A thing that is
  running and visible does not need to have been asked about; a thing that is running and
  invisible is what this subsystem exists to prevent.**
- **A job with no lane belongs to whoever asks** — the rule `Pending` already keeps for questions.
  `list` matched the lane exactly, so a spawn whose hook carried none filed the job under `""` and
  every window then matched nothing: a background command running, listed nowhere, with the
  agent's reply saying Keel was watching it.
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
  workload placement rules. `team` is what every project gets whatever its template, and what
  `/api/adopt` writes into an existing one: the seven agents this file's own pipeline runs,
  generalised; the lifecycle and the engineering rules for `CLAUDE.md`; and a `frontend` skill
  where there is a frontend. `agent/no-reviewers` asks for exactly that set, and
  `review::tests::adopting_clears_the_team_finding` fails if the scanner's list drifts from it. Pure functions from a project name to a list of files: no HTTP, no
  tokio, no git.
- `keel-harness` — `claude` supervision and trust quarantine.
- `keel-mcp` — the tool surface.
- `keel-providers` — GitHub, Cloudflare.
- `keel-workspace` — reads Claude Code's own state (sessions, skills, plugins, agents, commands,
  hooks, MCP servers). Read-only, and the listing never surfaces session message bodies — see
  non-negotiable 7. `tail()` is the one path that reads a conversation, on an explicit ask, and it
  is also how a session running outside Keel is watched: see "One reader".
- `desktop/` — the Tauri app: a page and a supervisor of daemons. A client, and nothing else.

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

## How a request becomes a change

Every request that changes the product runs this pipeline, in this order, and the person sees one
report at the end of it — not a stream of half-decisions along the way. The stages are the agents
in `.claude/agents/`. They run **one after another**, never in parallel, because each one reads
what the last one returned; and what a stage returned is handed to the next **verbatim**, because
a subagent starts with an empty context and a paraphrase is where a scope quietly grows.

1. **`pm`** — given the ask in the person's own words. Returns build / change / cut / not yet,
   and the smallest version worth shipping with its out-list. *Build* and *change* carry on, to
   the scope the PM set. **`cut` and `not yet` stop the pipeline and go back to the person**, with
   the reasoning: this is the one early exit, because hours spent building what the PM said not to
   build is the expensive way to have that conversation. They can overrule it in one word.
2. **`designer`** — given the ask and the PM's scope, whenever anything a person reads changes: a
   view, a label, a refusal's wording, CLI output. Returns the states table, the layout, the copy.
   Skipped — and said to be skipped — when nothing visible moves.
3. **`principal`** — given the ask, the scope and the design. Reviews both for what they missed
   (a state the daemon cannot actually report, a scope that breaks an invariant), then returns the
   technical design: the modules touched, the shared primitives to use, the invariants at risk,
   and the tests that will pin it. If it says the scope or the design cannot stand, that goes back
   through the stage that owns it once, not around it.
4. **Implementation** — the main loop, not a subagent, so the work is visible as it is written.
   It follows the principal's design; a deviation is written down with its reason, not made
   silently. Done means `make check` is green, then `reviewer` on the diff — and `security` or
   `reliability` when their descriptions say the change is theirs.
5. **`qa`** — given the ask, the scope and what changed. *Do not ship* means fix and run `qa`
   again, **twice at most**; after that the person gets the change and the open bug list rather
   than a loop with no end, because a pipeline is a wait and every wait ends.

**What the person is shown** is one report: what was asked, what the PM scoped in and out, what
the designer and the principal decided in a line each, the files changed, the gate's result, QA's
verdict with what it covered and did not, and anything any stage was overruled on. Faithfully — a
stage that could not run (a build the machine cannot do, an app QA may not quit) is named as not
run, never implied.

**What does not go through it:** a question, an explanation, running or launching something, git
and release chores, edits to docs and to these agents, and a change with no decision in it — a
typo, a rename, a version bump. Say in one line that it was skipped. "Just do it" and "skip the
pipeline" skip it; naming stages ("no PM", "QA only") runs the ones named.

## Verification

`make check`. Dogfood with `make scan`, and against `../A2ABaseAI` for a repo with real CI and tests.

## Production

The production checklist the reviewers enforce is in `docs/PRODUCTION.md`. It applies.
