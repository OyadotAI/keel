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
