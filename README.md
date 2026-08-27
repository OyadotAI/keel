# Keel

**The local IDE that makes a repo shippable.**

Keel opens a repository you already have, tells you honestly how ready it is to be worked on by an
agent and deployed to production, fixes what's missing, provisions Cloudflare, and gives you a chat
loop where GitHub state, both environments and a live preview are always on screen.

It is not an editor. Keep using yours — Keel works on the same directory.

## Status

Milestone 0. `keel scan` works end to end; everything else is scaffolded.

```
keel scan .              # readiness report
keel scan . --json       # machine-readable
keel scan . --strict     # exit 1 if anything critical or high is outstanding
```

## Why it exists

Today's AI app builders are built for non-engineers: a React front-end over a managed backend, hosted
by the vendor, and that's the end of it. **98%** of 1,072 scanned vibe-coded apps had a security
flaw. **11%** of a 20k-app sweep leaked database credentials.

The good agents — Claude Code, Cursor, Codex — write real code and stop at the repo edge. None of
them provision infrastructure.

Keel is the missing half.

## What it checks

| Dimension | Question |
|---|---|
| Agent legibility | Can an agent understand this repo without guessing? |
| Verifiability | Is there anything that would contradict an agent that thinks it's done? |
| Workers compatibility | Will this actually run on Cloudflare? |
| Runtime contract | Health checks, graceful shutdown, config, logging |
| State placement | Is the data somewhere that can hold it? |
| Environment hygiene | Are dev and prod genuinely separate? |
| Deployability | CI, no committed secrets, reproducible builds |
| Observability | Would you find out if it broke? |
| Security | Dependencies, secrets, and untrusted agent config |

Every finding carries a fix. A finding without one is a bug in the scanner.

## How it runs the agent

Keel drives **your installed `claude`**, so your existing subscription covers it. No API key.

The agent has no shell. `--permission-mode dontAsk` plus `--strict-mcp-config` mean it can only act
through Keel's own tools — which is what makes the audit log complete and the approval gates real
rather than advisory.

Before any agent runs, Keel quarantines repository-supplied `.claude/settings.json` and `.mcp.json`.
See [`docs/guardrails.md`](docs/guardrails.md) for why that is not optional.

## Development

```
make check    # fmt, clippy -D warnings, tests
make scan     # scan this repo with the freshly built binary
```

## License

Apache-2.0
