# Changelog

All notable changes to Keel are recorded here. Format follows Keep a Changelog.

## [Unreleased]

### Added
- Open-source contribution guide, pull request template, bug/feature/question issue forms,
  security-reporting policy, code of conduct, support guide, and shared editor settings.
- README coverage of existing Claude session history and live following across compatible local
  front-ends, plus the free/open-source commitment, sponsor, and lead maintainer.
- `keel scan` — repository readiness report with deterministic scoring across nine dimensions.
- Seven checks: untrusted agent config, shared environment bindings, Workers compatibility,
  committed secrets, missing tests, missing CI, missing agent instructions.
- Workers compatibility check: a missing `compatibility_date` (Wrangler will not deploy), `node:`
  imports without the `nodejs_compat` flag, `nodejs_compat` enabled below its 2024-09-23 minimum
  date, and imports of Node builtins the runtime never provides. Scoped to the sources reachable
  from the config's `main`, so a monorepo's Node server and test files are not judged against the
  Workers runtime.
- Trust quarantine for repository-supplied `.claude/` and `.mcp.json`, run before any agent starts.
- Locked-down `claude -p` invocation builder (`dontAsk` permissions, strict MCP config).
- Tool catalog with environment-scoped deploy tools.
- Golden-path templates: dual-environment `wrangler.jsonc`, promotion-aware CI, non-root Dockerfile.
- `keel trust` — quarantine repository-supplied agent config before any agent runs.
- `keel workspace` — inventory of everything Claude Code knows about a repo: sessions, skills,
  plugins, subagents, commands, hooks and MCP servers, each labelled by scope.
- `keel sessions` — recorded sessions with generated titles, message counts and resume ids.
- `keel serve` — the local IDE. Loopback-only axum server with a single embedded HTML page;
  Overview, Readiness, Sessions, Skills and Trust views, deep-linkable by URL hash.
- Untrusted marking: project-scoped hooks and MCP servers are flagged because they arrived with the
  repository rather than from the user's own configuration.

### Fixed
- App-launched daemons no longer initialize a separate reporting client that bypasses the native
  Privacy switch. Reporting copy no longer describes persistent usage identifiers as anonymous.
- Project trust and approval rules now stay in private machine-local storage; repository-supplied
  permission files cannot authorize themselves. Legacy grants require explicit reapproval.
- Build and installer permissions are suggested, not automatically granted from project files.
  Background commands require authorization before the daemon starts them.
- Session grants are scoped to both the project and conversation. Concurrent permission saves
  preserve existing decisions, and corrupt or linked authorization records fail closed.
- Local audit captures and project settings are excluded from the public snapshot; the launch kit
  is tracked, privacy behavior is documented, and PR CI now builds/tests the native macOS app.
- CI could not build the workspace on its Linux runner. `wry`/`tao` resolve GTK and WebKit through
  pkg-config on Linux, which `ubuntu-latest` does not ship, so `glib-sys` failed its build script
  before any check ran. Both jobs now install `libwebkit2gtk-4.1-dev`,
  `libjavascriptcoregtk-4.1-dev`, `libsoup-3.0-dev`, `libgtk-3-dev` and `libxdo-dev` (the
  4.1/libsoup3 line, which is what `webkit2gtk-sys` 2.0 requires; `tao` links `libxdo` directly, so
  it is needed to link but not to check).
- `gui.rs` did not compile off macOS: `Menu::init_for_nsapp` hands the menu bar to AppKit and exists
  only in muda's macOS build. Gated on `target_os = "macos"`, so the rest of the module is still
  type-checked on Linux.
- `unnecessary_sort_by` in `connect.rs`: `sort_by` with a hand-written key comparison became a
  clippy error on a newer toolchain than the one it was written against. Now `sort_by_key`.
- `cargo fmt --all -- --check` failed on 43 hunks across 14 files, the backlog from before CI
  existed. Reformatted `crates/keel/src/{api,approve,aws,clitools,fsops,infra,main,mcp,names,
  permissions,prefs,project,serve}.rs` and `crates/keel-workspace/src/sessions.rs`. No behaviour
  change.
