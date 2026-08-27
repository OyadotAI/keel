# Architecture

## Shape

```
keel                       # single binary; serves the UI on http://127.0.0.1:7777
├── keel-scanner           # readiness checks. No network, no agent, no credentials.
├── keel-harness           # spawns and supervises `claude -p`; trust quarantine
├── keel-mcp               # Keel's tool surface, served over loopback
├── keel-providers         # github / cloudflare
├── keel-generator         # golden-path templates and workload placement
└── ui                     # React + Vite, embedded in the binary
```

## Why a local binary

Three reasons, in order of importance:

1. **It does not ask anyone to switch editors.** Keel works on a real directory; your editor keeps
   working on the same files. Keel owns what nobody else owns — ship, run, observe.
2. **Credentials never leave the machine.**
3. **Marginal cost approaches zero.** The user's own `claude` covers inference and Workers scale to
   zero, so Keel carries neither the inference COGS nor the idle infrastructure bill that have made
   this category financially brutal for hosted competitors.

Rejected: forking VS Code (Cursor and Windsurf carry that cost because the editor *is* their
product) and Electron.

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
repo produce byte-identical reports. The corpus tests depend on that.

## Where state is allowed to live

`keel-generator::place` encodes the boundary. The load-bearing fact: **Cloudflare Container disk is
ephemeral and resets to the image on every restart**, so durable state never goes there. D1 is a
single-writer database at roughly 50 writes/sec; above that Keel reaches for Hyperdrive in front of
a managed Postgres rather than pretending D1 scales.
