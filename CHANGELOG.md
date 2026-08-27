# Changelog

All notable changes to Keel are recorded here. Format follows Keep a Changelog.

## [Unreleased]

### Added
- `keel scan` — repository readiness report with deterministic scoring across nine dimensions.
- Six checks: untrusted agent config, shared environment bindings, committed secrets, missing tests,
  missing CI, missing agent instructions.
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
