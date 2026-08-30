# Architecture

## Shape

```
Keel.app                    # native SwiftUI task browser
├── app/Sources/KeelApp     # task, evidence, review and terminal UI
└── keel                    # local Rust daemon on 127.0.0.1
    ├── keel-scanner        # readiness checks; no network or credentials
    ├── keel-harness        # agent invocation and trust quarantine
    ├── keel-mcp            # typed tool surface over loopback
    ├── keel-providers      # GitHub and Cloudflare integrations
    ├── keel-generator      # golden-path templates and workload placement
    └── keel-workspace      # reads Claude Code state; read-only
```

## Why a local binary

Three reasons, in order of importance:

1. **It does not ask anyone to switch editors.** Keel works on a real directory; your editor keeps
   working on the same files. Keel owns what nobody else owns — ship, run, observe.
2. **Credentials never leave the machine.**
3. **Marginal cost approaches zero.** The user's own `claude` covers inference and Workers scale to
   zero, so Keel carries neither the inference COGS nor the idle infrastructure bill that have made
   this category financially brutal for hosted competitors.

Rejected: forking VS Code and Electron. Keel is a native macOS application and deliberately leaves
direct file editing to the engineer's existing editor.

## Why drive the CLI rather than embed an SDK

The Claude Agent SDK would mean shipping a second runtime and asking for an API key. Driving the
installed `claude` binary means the user's existing subscription applies. The cost is that Keel
cannot use SDK hooks, so **the guardrails live in the tool surface instead** — which is arguably
stronger, since a denied tool cannot be worked around by a prompt.

See `docs/guardrails.md` for what that buys and what it costs.

## The harness pattern

This maps one-to-one onto `A2ABaseAI/backend/app/agent_ide/`, which is a working implementation of
the same idea against a different CLI:

| A2ABaseAI (opencode) | Keel (claude) |
|---|---|
| `opencode_runtime.py` — spawn, generate config, reap idle | `keel-harness::invocation` |
| `mcp_server.py` — tools over MCP | `keel-mcp` |
| `bridge.py` — events → `content`/`status`/`change` frames | `keel-harness::bridge` |
| built-in tools disabled | `--permission-mode dontAsk` |

## Scanner independence

`keel-scanner` depends on nothing else in the workspace and touches no network. That is a
deliberate constraint: the scan ships and earns trust before Keel is handed a cloud credential, and
it stays safe to run against a repository nobody has reviewed.

Findings are ordered deterministically and scoring is a simple total, so two runs over an unchanged
repo produce byte-identical reports. The corpus tests depend on that. It is also why platform facts
with a date — `nodejs_compat` requiring a compatibility date of 2024-09-23 or later — are compiled-in
constants rather than comparisons against a clock.

The Workers-compatibility check scopes itself to the sources reachable from the Wrangler config's
`main`, not the whole tree. A monorepo legitimately contains a Node server, build tooling and tests
that use `node:fs` and never go near the runtime; judging those against Workers would be wrong
rather than merely noisy.

## Where state is allowed to live

`keel-generator::place` encodes the boundary. The load-bearing fact: **Cloudflare Container disk is
ephemeral and resets to the image on every restart**, so durable state never goes there. D1 is a
single-writer database at roughly 50 writes/sec; above that Keel reaches for Hyperdrive in front of
a managed Postgres rather than pretending D1 scales.

## Reading Claude Code's own state

`keel-workspace` exists because Claude Code's configuration surface is real, consequential and
invisible. Sessions are JSONL under `~/.claude/projects/<cwd-with-slashes-as-dashes>/`. Skills and
subagents are Markdown in two scopes. Plugins are a JSON index. Hooks live in settings files and run
shell commands.

Two rules shape the crate:

**Every discovery degrades to an empty list.** A machine with no plugins is not an error, and
neither is one where the layout has moved on. A malformed file yields nothing rather than taking the
listing down, and a truncated transcript line is skipped rather than hiding the conversation.

**Message bodies never leave the transcript.** Sessions are summarised to title, counts and
timestamps. A transcript contains everything the user has ever said in that repository; reading one
to render a list is not permission to display it. There is a test asserting no conversation content
appears in the serialised form.

Scope is the load-bearing distinction. `Project` configuration arrived with the repository, authored
by whoever wrote it — which is not necessarily the person running it. That is what `untrusted_count`
counts, what the UI marks with `!`, and what `keel trust` quarantines.
