<h1 align="center">Keel</h1>

<p align="center"><b>Ship agent-written code you can trust.</b></p>

<p align="center">
An open-source native macOS ADE for product engineers. It runs <i>your</i> Claude Code locally,
isolates each task on its own branch and worktree, and shows the evidence required for a human
merge decision.
</p>

<p align="center">
Three things are the product: <b>visibility</b> into what the agent just did, <b>git management</b>
that keeps the history mergeable, and <b>multi-agent</b> lanes that do not step on each other.
Everything else is in service of those.
</p>

- **Local.** A macOS app talking to `127.0.0.1`. Your code never leaves the machine.
- **Your Claude account.** It drives the `claude` you already have, so your subscription covers the
  agent. No API key, no second bill, no proxy.
- **One task, one identity.** An isolated lane binds the conversation, provider process, branch,
  worktree and evidence; when a policy demands isolation, failing to get a worktree stops the task
  rather than falling back to the shared checkout. A lane can also be opened *sharing* the working
  tree — for reading and planning beside one that is editing, which is what that mode is for.
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
no network.

The daemon runs on its own — `cargo install --path crates/keel`, then `keel serve . --no-open` —
but it is an HTTP surface, not a UI. The window is the Mac app; there is no page to open. (Without
`--no-open` it still opens a browser at `/`, which nothing serves any more. That is a bug, not a
UI.)

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
make check                       # fmt, clippy -D warnings, both test suites, the bundle budgets
make app && open dist/Keel.app   # the ADE, on your own checkout
cargo run -- serve . --no-open   # the daemon alone, to curl at
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
- **Windows and Linux** — `keel serve` runs anywhere Rust does; the window does not, because it is
  SwiftUI and AppKit. Porting means a second front end against the same daemon, which is real,
  self-contained work and the daemon was split out to make possible.

The native app has unit and rendering tests for session restoration, streaming events, task
isolation, diff rendering and interaction invariants.

Comments here explain *why*, especially where a platform constraint drove the design. A PR that
does the same is doing the thing this project values most.

## License

Apache-2.0
