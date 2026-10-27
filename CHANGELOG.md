# Changelog

All notable changes to Keel are recorded here. Format follows Keep a Changelog.

## [0.4.36] - 2026-10-27

### Fixed
- Synchronized Claude Code, Codex, and Antigravity lane names with their native session names,
  including terminal renames and names entered before session discovery.
- Retained named, Keel-known sessions in provider-aware History across close, resume, and restart,
  with serialized saves and revisions preventing stale refreshes from undoing newer names.

## [0.4.35] - 2026-10-24

### Fixed
- Restored Antigravity lane and team-member activity through independent child work, restarts,
  and pending timers without changing turn execution or transcript ordering.
- Corrected cached input and Antigravity output accounting, refreshed reported usage past pending
  steps, and explained parent-only totals and latest-context snapshots in the footer and inspector.
- Reported native Codex approval and question states in lane indicators, waiting badges, and
  background notifications, with reconnects restoring requests quietly.
- Applied project trust to Antigravity terminal permissions and permitted MCP browser actions
  across agents, while preserving explicit denies, ask rules, and browser isolation.

## [0.4.10] - 2026-10-16

### Added
- Added Markdown Preview and Split views with headings, tables, task lists, highlighted code
  blocks, relative document links, heading anchors, and local images. Previews refreshed after
  agent edits, and Cmd/Ctrl+Shift+V toggled the preview while preserving paste in text inputs.

### Changed
- Matched Source Control, Graph, editor, and terminal panels to Keel's rounded workspace surfaces.

### Fixed
- Bounded Markdown rendering and local asset reads, preserving checkout boundaries and the
  existing SVG download policy.
- Kept Code view available when returning from a Markdown preview or navigating to a source line.

## [0.4.9] - 2026-10-16

### Fixed
- Kept commit messages and branch badges visible when a dense Git graph was wider than the
  sidebar. The branch graph scrolled horizontally within its own column, while commit labels
  stayed in view and vertical scrolling kept nodes aligned with their commits.
- Highlighted the selected graph commit and supported keyboard navigation through its rows.

## [0.4.8] - 2026-10-16

### Added
- Added Coding Mode with a file explorer, code and diff tabs, a conversation pane, and an
  integrated terminal that retained its session when hidden or when switching lanes.
- Added VS Code navigation shortcuts for quick file opening, line navigation, file search,
  commands, the explorer, source control, and terminal visibility.
- Added a source control commit graph with branch and remote badges, sync controls, and commit
  review in the center pane so changes and history could be reviewed together.
- Added syntax highlighting to source previews and Git diffs, including Go, YAML, TOML, SQL,
  C/C++, Java, C#, Ruby, PHP, and Dockerfiles.

### Changed
- Renamed the workspace layouts to Agentic Mode and Coding Mode, and Members to Keel Agents.
- Hid the Chat/Terminal interface switch in Coding Mode and widened its resizable sidebar.

### Fixed
- Fixed clipped filenames in Go to File and kept navigation dialogs readable in narrow windows.
- Preserved staged and unstaged diff selection and kept code previews current after agent edits.
- Bounded file previews and commit history reads, and protected pulls with a checkout writer claim.

## [0.4.2] - 2026-10-12

### Changed
- Readiness findings are grouped by session, folder, and global scope, with score-row filters and
  counts for each scope.

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
- Kept Codex terminal session discovery working across date-folder changes and selected the
  earliest eligible unclaimed conversation across the launch window.
- Skipped already claimed Antigravity conversations before selecting a terminal session.
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
