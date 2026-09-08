# Guardrails

This describes the protections in the app's chat runtime, implemented in `crates/keel/src/agent.rs`.
The separate `keel-harness::Invocation` API and `keel-mcp` tool catalog are prototypes; their tests
do not establish restrictions on a chat turn that does not use them.

## Runtime permissions

Claude chat uses the installed CLI's built-in tools with `acceptEdits` or `plan`, Keel's permission
settings and approval hooks, and the `ask_user` MCP server. It is not confined to the prototype
Keel tool catalog. Trusted projects can bypass command approval. Codex uses its own CLI sandbox:
`workspace-write` for Auto and `read-only` for Plan, including resumed turns. The providers do not
have identical approval behavior. Missing or unrecognized modes use the read-only lifecycle.

## Repository config quarantine

Before launching a provider, the daemon calls `keel-harness::quarantine` and checks that the
executable repository config paths were removed. A failure or a conflict with an earlier
quarantine refuses the turn before the provider starts. Inspect `.claude/`, `.mcp.json`, and
`.keel/quarantine/` to resolve it. Quarantine moves files; it does not delete the originals.

This covers the paths recognized by `keel-harness::trust`, not arbitrary scripts a person later
approves. The scanner reports repository agent config separately as
`security/untrusted-agent-config`; scanning alone does not execute its hooks.

## Generated environment separation

The generator creates separate resources per environment, and the scanner reports shared
bindings. These checks do not constrain arbitrary deployment commands an agent is allowed to run,
and a finding does not automatically repair a deployed environment.

## Protections not implemented in chat

The prototype invocation and tool catalog describe environment-scoped mutation approval. The
chat runtime does not enforce that production contract. Artifact-preserving promotion, typed
production confirmations, automatic token scoping/rotation, and a hard spending ceiling are not
implemented guarantees. Token usage and provider-reported cost are recorded; reporting a cost
after a turn is not a spending limit.

## Approval happens in the IDE

`--permission-mode acceptEdits` lets the agent write files but **not** run arbitrary shell commands;
apart from a read-only command set, those still need an allow rule. A headless `claude -p` session
has nobody to prompt, and this CLI has no `--permission-prompt-tool`, so a denied command simply
fails — which is why an agent could write a landing page and then be unable to install or build it.

Keel therefore owns the allowlist. A denial surfaces in the conversation with the exact command.
Allow once approves only the held invocation and stores no rule. Longer-lived session rules,
project rules, and project-wide trust are explicit choices under More options; Deny stays visible.
Approved rules are passed to `claude` via `--settings` on every subsequent run. Approval stays a
human act; it happens in the IDE instead of at a terminal prompt.

Rules are derived from the call rather than hardcoded per tool, so this works for any command.
Verified: `docker --version` returns "This command requires approval", and after approving
`Bash(docker *)` the same request returns the version.

Project trust and rules live in `~/.keel/permissions/`, outside repository-controlled files,
in private records keyed to the canonical project path and directory identity. Missing, corrupt,
linked, or non-private records grant nothing. Concurrent daemons lock the record while updating it.
Repositories cannot import their own `.keel/permissions.json`: legacy files are ignored, not
automatically migrated. Reapprove needed rules or trust in Settings → Permissions on each Mac.
Session rules are scoped to both project and conversation, and last until Keel restarts. The
project's own build and test commands are *suggested* rather than pre-approved — one click each,
because the point is that a person decides.

Background commands pass the same authorization boundary before Keel starts them. Because Keel
runs monitored commands without Claude's shell parser, a prefix suggestion alone cannot authorize
them: approve the exact invocation, explicitly allow all Bash commands, or trust the project.
This protects against repository-supplied grants; it is not an OS sandbox against other programs
already executing as your user. Explicit verification, installers, and tools you approve can run
repository code. Review a repository before running its checks or trusting it.

## Installing tools and signing in

First launch and Settings share the Claude setup flow. Installation runs Anthropic's native
installer only after an Install action; the command is shown before it runs. Sign-in invokes
`claude auth login`, streams its output, and exposes HTTPS links for browser completion. Keel
does not collect the account password. Authentication is checked with `claude auth status`, not
inferred from stale account metadata. These commands are documented in the
[official CLI reference](https://code.claude.com/docs/en/cli-usage).

A zero exit code is followed by a fresh availability/authentication check. Failed commands and
premature stream closure remain visible errors. Cancelling setup closes the stream and ends the
owned process group; cancellation cannot roll back files an installer already changed.

Other developer tools are optional. Settings names the installer command and asks before running
it. Homebrew setup and cask installers use the integrated terminal where password prompts can be
answered. No blanket installation or automatic account login runs merely because Settings opens.

## Evidence over assertion

The agent does not grade its own work. After every non-plan turn Keel runs the project's own check
command and attaches the exit code to that turn. This is the gate the research ranks highest for
cost-to-value, and it is the one that would have caught a landing page reported as finished while
`bun run typecheck` was failing.

Keel never invents a check. It uses the one the project declares — a `check` target, package
scripts, or the language default — and says "not verified" when there is none, rather than implying
a pass it did not observe.

## Plan mode

The chat panel offers Plan and Auto. Plan passes `--permission-mode plan`, and it holds: asked to
create a file, the agent attempts `Write`, is refused, and produces a plan instead — verified, with
no file on disk afterwards. An unrecognised mode falls back to Plan, because the safe default is the
one that cannot change the repository.

Getting *out* of it is the one thing headless plan mode gives nobody. There is no `ExitPlanMode`
in `claude -p` — its plan-mode tool list has no plan tool at all, verified against 2.1.251 — and an
MCP tool cannot stand in for one, because plan mode refuses every MCP call outright: `Cannot call
mcp__keel__ask_user while in plan mode`. That is Claude Code, and nothing on our command line
changes it. It is also why `ask_user` never worked in a planning turn, which is a question card
that could not appear rather than one nobody answered.

So Keel uses the channel Claude Code already drives. Every plan turn opens with a `plan_mode`
attachment naming a `planFilePath` under the Claude home, the turn is told to write its plan there,
and that write is permitted where everything else is refused. Keel hooks `Write` already, so it
recognises that path, holds the call, and draws the plan with an Approve button. Both answers deny
the write — Keel has the plan's text, so the file is one nobody needed — and neither is a refusal.
Approving cannot mean "carry on", because that turn cannot write a file whatever it is told: it
ends, and the build is a second turn in `acceptEdits` resumed on the same conversation, which is
where the plan is.

The system prompt says the same thing to the agent, including what to do with a question it cannot
ask: put it in the plan, as the decision made and the one it would rather have checked.

## What the server will read

The file reader allows two roots: the open repository, and the user's Claude Code home. The second
is deliberate — skills, subagents and commands live under `~/.claude`, and opening one is the point
of the skills panel. Everything else is refused, verified against `/etc/passwd` and `~/.ssh/id_rsa`.

The folder browser is narrower still: it lists directories inside `$HOME` only. Keel binds to
loopback, but any page in the browser can reach a loopback server, so an unbounded filesystem
enumerator would be a real disclosure rather than a theoretical one.

## Credentials

GitHub and Cloudflare tokens live in the OS keychain (`keyring`), keyed under `dev.keel`, and are
sent only to their own APIs. Keel prefers an explicitly connected GitHub token and otherwise reads
`gh auth token` per call — never storing that one, so revoking `gh` revokes Keel.

Every token is verified before it is stored. Storing first means the failure surfaces later, in the
middle of some unrelated operation, with nothing pointing at the credential as the cause.

## Rewind and Git operations

Snapshots include tracked files, even if a later ignore rule matches them, plus non-ignored
untracked files. Rewind changes the working copy through a temporary index and preserves the
user's staged version. It first records an undo snapshot and refuses collisions with ignored
files. Ignored directories are checked conservatively: move a conflicting directory aside before
rewinding. Snapshots are unreferenced Git trees and can eventually be collected by Git.

A normal lane discard refuses if Git cannot determine what would be lost. A workspace commit
that succeeds in some repositories and fails in others reports both; completed commits remain
intact so a retry can finish the remaining repositories.

## Multiple windows and checkouts

Writer turns hold an operating-system advisory lock on the checkout directory through startup,
provider execution, verification, and commit. Separate Keel daemons cannot claim that same
directory for editing at once. Symlink aliases and subdirectories resolve to the checkout;
separate linked worktrees remain independent. A missing or unsupported lock refuses the turn.
Read-only planning turns do not take the writer lock.

This coordinates participating Keel daemons, not arbitrary editors or terminal commands. An
umbrella workspace locks its selected directory, not every repository beneath it. Agents running
from such a folder and independently opened child repositories are not mutually excluded.

Detaching a lane preserves its session identity, existing checkout, provider/model, permission
mode, and branch preferences. Transcript and file requests use the retained checkout rather
than allowing stale transcript metadata to redirect one of them elsewhere.

## Stop, not abandon

Stop records cancellation even before the provider has a process ID. Once running, it requests
**SIGINT** for the provider's process group. A disconnected chat also terminates and reaps the
provider, including one producing no output, before releasing its checkout. This does not promise
that every third-party process can undo its work on interruption.

The app waits for a pending Stop request before sending a replacement turn in that lane. A
cancelled stream cannot finish its replacement, and late approval responses cannot resurrect a
stopped turn's cards. Closing a window stops its lane readers and project-event subscription.
