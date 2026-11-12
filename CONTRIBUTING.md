# Contributing to Keel

Keel is an opinionated ADE for developers and product engineers who love Claude Code's agent
and want a better experience around it. It is free, open source, and will always be free to use.

You do not need to know the whole codebase to contribute. A reproducible bug, a clearer error,
a keyboard improvement, or a better setup instruction is a useful contribution.

## Choose a contribution

- Report bugs and propose features through the [issue forms](https://github.com/OyadotAI/keel/issues/new/choose).
- Send small, focused fixes directly as pull requests against `main`.
- Discuss new dependencies, larger features, architecture changes, and changes to permissions or
  data handling before investing in an implementation.
- Look for issues marked `good first issue` or `help wanted` when available. Ask in an issue if
  you need a starting point; labels are not a prerequisite for contributing.

Use [Support](SUPPORT.md) for setup questions and [Security](SECURITY.md) for private reports.
Everyone participating follows the [Code of Conduct](CODE_OF_CONDUCT.md).

## How to build your first local app

### Prerequisites

- macOS or Windows to run the app; the daemon and its tests also build on Linux.
- Current stable Rust, including Cargo, rustfmt and Clippy; Node.js and pnpm; Git.
- A signed-in Claude Code (or Codex, or Antigravity) is needed only for real agent runs, not for
  `make check`.

If Rust is managed by rustup, `rustup component add rustfmt clippy` adds the check tools.

1. Fork the source repository, clone your fork, and create a branch for your change:

   ```sh
   git switch -c fix/session-history
   ```

2. Run the app against a daemon built from your tree:

   ```sh
   make dev
   ```

   It installs the page's dependencies, stages the daemon beside the app, and starts the Tauri
   app with live reload. You do not need signing or release credentials to contribute.

### Troubleshooting

- **`pnpm: command not found`:** install pnpm (`npm install -g pnpm`), then run `make dev` again.
- **An agent is missing or signed out:** use Keel's setup screen. Do not paste account tokens into
  an issue, a test fixture, or a repository config file.
- **An old project asks for permissions again:** trust and approved rules are machine-local.
  Repository permission files are ignored. Review and reapprove needed rules in Settings → Permissions.

## How to verify a change

The complete gate is:

```sh
make check
```

It checks Rust formatting, runs Clippy and the Rust tests, then type-checks, lints, tests, bundles
and size-checks the app, runs the agent helper's tests, and tests the app shell. For faster
feedback while editing:

| Change | Focused check |
| --- | --- |
| Rust formatting | `cargo fmt --all -- --check` |
| Rust lint | `cargo clippy --all-targets -- -D warnings` |
| Claude session discovery and following | `cargo test -p keel-workspace sessions::` |
| The app's page | `cd desktop && pnpm test` |
| The agent helper (chat turns for Claude and Codex) | `cd desktop && pnpm test:runtime` |
| The app shell | `cd desktop/src-tauri && cargo test` |

Use `cargo fmt --all` to apply Rust formatting, and `cargo fmt` inside `desktop/src-tauri` for the
shell, which is its own workspace. Follow nearby conventions and `.editorconfig`; do not reformat
unrelated files.

`make evals` invokes real Claude sessions and can spend your account's usage. It is opt-in and is
not covered by a passing `make check`. For provider-protocol changes, say whether you ran it;
otherwise request maintainer help validating the live flow. Never add credentials to CI or fixtures.

## Pull request rules

1. **One clear problem per PR.** Explain the user-visible effect, link a related issue if one
   exists, and leave unrelated cleanup out. A descriptive title is enough; no special commit format is required.
2. **Prove the behavior.** Add a regression test for a bug or explain why that is impractical.
   Report exact checks run, results, and anything not tested. Do not describe fixtures as live acceptance.
3. **Protect people's work.** Changes to Git, process lifetime, approvals, credentials, or
   configuration need failure-path coverage. Unknown repository state is not permission to delete it.
4. **Show UI changes.** Include before/after screenshots or a short clip. Check light/dark mode,
   narrow windows, keyboard focus, and accessibility labels for changed controls. Redact private content.
5. **Keep setup understandable.** New tools need detection, an actionable install/login path,
   visible errors, and tests. Explain new dependencies and why existing components are insufficient.
6. **Update the docs.** Update user instructions when behavior changes. Add a short entry to the
   existing Unreleased changelog for user-visible changes. Do not bump versions or create release tags.
7. **Own AI-assisted work.** AI-assisted contributions are welcome. You must understand the diff,
   check its provenance, review it yourself, and verify it. Note material AI assistance in the PR;
   generated volume does not substitute for evidence.

The [PR template](.github/PULL_REQUEST_TEMPLATE.md) prompts for these details. Mark irrelevant
items as not applicable with a short reason; documentation-only changes do not need an app rebuild.

## Review and maintenance

**Lead maintainer: Mohamed Alkiswani** ([mk@getoya.ai](mailto:mk@getoya.ai)).
Keel is sponsored by [OyaBrowser.com](https://oyabrowser.com).

Maintainers decide scope and merge pull requests. A passing test suite is necessary for code
changes but is not automatic approval. Review favors user-work safety, a fast and legible UI, understandable
behavior, and compatibility with existing Claude sessions. Disagreements should focus on evidence
and trade-offs; explain a concern in the PR or related issue.

Please give reviewers time. Maintainers may ask to split a change, request tests, or decline a
feature that does not fit the product. There is no guaranteed review or support response time.

Release publication is maintainer-only. Do not run `make release` for a contribution: it changes
versions, creates a commit, pushes code, and tags a release. Signing and publishing credentials
never belong in a pull request.

## License and orientation

Submit only code and assets you have the right to contribute under the project's [license](LICENSE.md).
Keep applicable copyright notices and identify third-party code or assets and their licenses.

Review [Privacy](docs/privacy.md) before adding telemetry or diagnostics. Never attach prompts,
repository names, private paths, or source content to reporting events.

[Architecture](docs/architecture.md) explains the modules. [Guardrails](docs/guardrails.md) describes
runtime protections. The [launch kit](docs/launch.md) explains how to regenerate product media.
