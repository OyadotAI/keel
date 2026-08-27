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

## Known gap: the chat panel is not yet behind this contract

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
