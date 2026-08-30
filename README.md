<h1 align="center">Keel</h1>

<p align="center"><b>Ship agent-written code you can trust.</b></p>

<p align="center">
An open-source native macOS ADE that runs <i>your</i> Claude Code locally, isolates each task on its
own branch and worktree, and shows the evidence required for a human merge decision.
</p>

- **Local.** A macOS app talking to `127.0.0.1`. Your code never leaves the machine.
- **Your Claude account.** It drives the `claude` you already have, so your subscription covers the
  agent. No API key, no second bill, no proxy.
- **One task, one identity.** A tab binds the conversation, provider process, branch, worktree and
  evidence. Failed isolation stops the task instead of falling back to the shared checkout.
- **Nothing hidden.** Live diffs, tool calls, command intent, approvals and the project's own checks.
- **Human-owned merge.** A review packet blocks merge when checks fail, evidence is missing or the
  change exceeds its approved scope.

```
Every task. One branch. Every action visible. Engineering quality before merge.
```

## Install

```
git clone <this repo> && cd keel
make app && cp -R dist/Keel.app /Applications/
```

Or grab `Keel.dmg` from a release — signed, notarised and stapled, so it opens with no warning and
no network. CLI only: `cargo install --path crates/keel`, then `keel serve .`

## The tour

**Every file a turn touched, in one screen** — stacked diffs, live, instead of `git diff`
afterwards. Reopen a session from last week and the same panel fills from its transcript: what it
changed, and every command it ran with the output it got.

**A real terminal, tabbed** — each tab named after what is running in it, so you can tell which one
is the build you are waiting on. `⌘T` for another.

**Everything Claude Code keeps in dotfiles** — sessions, skills, subagents, plugins, hooks, MCP
servers — visible and managed through `claude`'s own commands, with anything that arrived *with the
repository* marked, because a project-scoped hook was written by whoever wrote the repo.

Also: approvals answered in the conversation instead of a prompt that scrolled past, the project's
own gate run after every turn with the failures parsed into clickable problems, a preview that can
hold a login, and a readiness scan when you want to ship.

→ [**The whole thing, with the reasons**](docs/features.md) · [guardrails](docs/guardrails.md) ·
[architecture](docs/architecture.md)

## Contributing

Wanted. It is deliberately easy to work on.

```
make check                # fmt, clippy -D warnings, tests — the whole gate, ~5s
cargo run -- serve .      # the IDE, on your own checkout
```

**Native SwiftUI.** No Electron, browser shell, JavaScript build, bundler or `node_modules`. The Rust
daemon owns repository, Git, terminal and agent effects; the Swift app owns the task-browser UX.

| Crate | Owns |
|---|---|
| `app/Sources/KeelApp` | Native task browser, evidence, review and terminal surfaces |
| `keel` | Local daemon: HTTP surface, agent session, terminal and Git |
| `keel-scanner` | Readiness checks. No network, no dependency on the rest |
| `keel-workspace` | Reading Claude Code's own state. Read-only |
| `keel-harness` | Supervising `claude`, and the trust quarantine |
| `keel-providers` · `keel-generator` · `keel-mcp` | GitHub and Cloudflare · templates · the agent's tools |

Good first PRs, each a genuinely small diff:

- **A readiness check** — one function in `keel-scanner`, plus its `Fix`. A finding without a fix is
  a bug: it turns the report into a lint run nobody acts on.
- **A gate Keel doesn't know** — `verify::detect` is match arms in priority order. Deno, Gradle,
  Mix: one arm, one test.
- **A CLI on the Connections screen** — `clitools::TOOLS` is a static table. An entry is an id, a
  binary, and the args for version, whoami, install and login.
- **Windows and Linux** — `keel serve` runs anywhere Rust does, and the window is `tao` + `wry`,
  which build everywhere; only macOS has ever been tried. Running it elsewhere and fixing what
  falls over is real, self-contained work.

The native app has unit and rendering tests for session restoration, streaming events, task
isolation, diff rendering and interaction invariants.

Comments here explain *why*, especially where a platform constraint drove the design. A PR that
does the same is doing the thing this project values most.

## License

Apache-2.0
