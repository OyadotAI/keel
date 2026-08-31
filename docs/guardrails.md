# Guardrails

This is a contract, not a wish list. Each item states what is enforced and where, so a reviewer can
check the claim against the code rather than trusting the prose.

## 1. The agent can only act through Keel's tools

**Enforced in** `keel-harness::invocation` — `--permission-mode dontAsk` makes Claude Code deny
anything not explicitly allowed, and `--strict-mcp-config` confines the session to the MCP servers
Keel passes. Keel allows no built-in tools, so `Bash`, `Edit` and `Write` are unavailable.

*An agent that cannot call `bash` cannot delete a database.* This is also what makes the audit log
complete: every effect the agent has on the world is a tool call Keel served.

Test: `invocation::tests::locks_the_tool_surface`.

## 2. Repository-supplied agent config is quarantined before anything runs

**Enforced in** `keel-harness::trust`.

Keel drives the user's installed `claude` so their subscription covers their usage. That rules out
`--bare`, whose own help text states that under it "OAuth and keychain are never read". Without
`--bare`, a `-p` session loads and acts on the repository's own `.claude/settings.json` hooks with
no workspace-trust dialog.

Keel's core flow is *clone a repo the user picked and point an agent at it*, so a hostile repository
would otherwise reach code execution the moment it is scanned. `--strict-mcp-config` covers a repo
`.mcp.json`; **hooks it does not cover**, which is why quarantine runs first.

Tests: `trust::tests::*`. Also surfaced to the user as scanner finding
`security/untrusted-agent-config`.

## 3. Autonomy is scoped by environment

**Enforced in** `keel-harness::invocation::Environment` and the tool catalog.

- `dev` — the agent acts freely. Deploy, migrate, delete, retry.
- `prod` — every mutating tool call requires human approval; irreversible ones (D1 migration, R2
  delete, token or DNS change) require typed confirmation.

Every deploy-family tool takes an explicit `env` with **no default**. An agent that must name the
environment cannot drift into production by omission.

Tests: `tools::tests::every_deploy_tool_is_environment_scoped`.

## 4. Environments never share state

**Enforced in** the generator (separate resources per env) and checked by
`keel-scanner`'s `env/shared-bindings`, which is `Critical`.

A dev binding resolving to a production resource id is the most common way a safe-looking action
destroys real data. Keel does not generate it and does not leave it in place.

## 5. Promotion ships the proven artifact

Promotion redeploys the exact version already validated in dev. It never rebuilds — rebuilding is
how dev and prod silently diverge — and never silently applies a migration.

Before promotion fires, the user sees: commit diff, binding diff, pending D1 migrations called out
separately, prod secret presence, and dev check status.

## 6. Spend has a hard ceiling

`--output-format json` returns `total_cost_usd` per invocation, which feeds a hard cap rather than
an alert. Cloud billing lags agent speed by roughly a day; a documented 2026 incident had an agent
provision $6,531 of infrastructure in 24 hours before anyone could see it.

## 7. Credentials never leave the machine

OS keychain only. Cloudflare tokens are provisioned scoped to a single project and rotated, because
Cloudflare has no OIDC or keyless deploy path as of August 2026.

## Approval happens in the IDE

`--permission-mode acceptEdits` lets the agent write files but **not** run arbitrary shell commands;
apart from a read-only command set, those still need an allow rule. A headless `claude -p` session
has nobody to prompt, and this CLI has no `--permission-prompt-tool`, so a denied command simply
fails — which is why an agent could write a landing page and then be unable to install or build it.

Keel therefore owns the allowlist. A denial surfaces in the conversation with the exact command and
three choices: allow the derived rule for this project, allow it for this session only, or refuse.
Approved rules are passed to `claude` via `--settings` on every subsequent run. Approval stays a
human act; it happens in the IDE instead of at a terminal prompt.

Rules are derived from the call rather than hardcoded per tool, so this works for any command.
Verified: `docker --version` returns "This command requires approval", and after approving
`Bash(docker *)` the same request returns the version.

Project rules live in `.keel/permissions.json`. Session rules last until Keel restarts. The
project's own build and test commands are *suggested* rather than pre-approved — one click each,
because the point is that a person decides.

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

Getting *out* of it is Keel's own tool. Headless `claude -p` offers no `ExitPlanMode` — its
plan-mode tool list has no plan tool at all, verified against 2.1.251 — so a planning turn's only
ways to deliver a plan were its own reply and a file on disk, and the one thing the person had to
decide on arrived as prose with nothing to click. `submit_plan` (the `keel` MCP server, offered in
plan mode alone) queues the plan into the same card a question uses. Approving cannot mean "carry
on", because that turn is under `--permission-mode plan` and cannot write whatever it is told: it
ends, and the build is a second turn in `acceptEdits` resumed on the same conversation, which is
where the plan is.

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

## Known gap: Auto mode is not yet behind this contract

Rules 1 and 3 describe `keel-harness::invocation`, which is built and tested. **The IDE's chat panel
does not use it yet.** It spawns `claude` with `--permission-mode acceptEdits`, so the agent has real
file access and can run commands.

The reason is not oversight: the locked-down surface depends on `keel-mcp` having an actual server
behind its tool catalog, and it does not. Until then, an agent restricted to Keel tools would have
no tools at all and could do nothing useful. Shipping a panel that *claims* containment it does not
have would be worse than shipping one that says what it is, so the chat header states the permission
mode in plain language.

Closing this gap means implementing the MCP server and switching the spawn to `Invocation::args()`.

## Stop, not abandon

`SIGTERM` makes the CLI exit 143 and abandon the turn in progress. The Stop button sends **SIGINT**,
which ends the turn cleanly. Getting this backwards loses work.
