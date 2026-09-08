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

- A Mac with macOS 15 or later to run Keel.
- Xcode 26 with its Swift 6.2 toolchain selected. The current native code uses isolated
  conformances that older Swift toolchains do not support. Building needs a Mac compatible with
  that Xcode version, even though the app's deployment target is macOS 15.
- Current stable Rust, including Cargo, rustfmt, and Clippy; Git; Python 3.
- Pillow for rendering the app icon. The isolated Python environment below keeps it local.
- Optional: ffmpeg for media generation. A signed-in Claude Code account is needed only for
  real agent runs, not ordinary unit tests.

Check the selected tools with `xcrun swift --version`, `rustc --version`, and `python3 --version`.
If Rust is already managed by rustup, `rustup component add rustfmt clippy` adds the check tools.

1. Fork the source repository, clone your fork using its GitHub Clone button, and enter the
   checkout. Create a branch for your change:

   ```sh
   git switch -c fix/session-history
   ```

2. Create the packaging environment and build the app:

   ```sh
   python3 -m venv .venv
   source .venv/bin/activate
   python3 -m pip install Pillow
   make app
   ```

   This produces `dist/Keel.app`. The build uses an available Developer ID or falls back to
   ad-hoc signing for local development. You do not need release credentials to contribute.

3. Open your build:

   ```sh
   open dist/Keel.app
   ```

   You now have a local Keel build. For subsequent UI edits, `make dev` builds and launches
   `dist/Keel-dev.app`. It quits the running Keel instance, so finish active work first.

### Troubleshooting

- **Swift rejects an isolated conformance:** check `xcode-select -p` and `xcrun swift --version`.
  Select your Xcode 26 installation in Xcode's Locations settings, then rebuild.
- **`No module named 'PIL'`:** activate `.venv` in this shell and install Pillow with the command above.
- **Bundle or resource tests fail after a bare `swift test`:** run `make app-test`. It builds the
  release bundle before tests that inspect its size, resources, and process behavior.
- **`make dev` says no bundle exists:** run `make app` once first.
- **Claude is missing or signed out:** use Keel's setup screen. Do not paste account tokens into
  an issue, a test fixture, or a repository config file.
- **An old project asks for permissions again:** trust and approved rules are now machine-local.
  Repository permission files are ignored. Review and reapprove needed rules in Settings → Permissions.

## How to verify a change

The complete macOS gate is:

```sh
make check
```

It checks Rust formatting, runs Clippy and Rust tests, builds the release app, and runs Swift tests.
PR CI also builds and tests the native app on a hosted macOS runner without release credentials.
Keep the packaging Python environment active. For faster feedback while editing:

| Change | Focused check |
| --- | --- |
| Rust formatting | `cargo fmt --all -- --check` |
| Rust lint | `cargo clippy --all-targets -- -D warnings` |
| Claude session discovery and following | `cargo test -p keel-workspace sessions::` |
| Native setup and command palette | `swift test --package-path app --filter SetupFlowTests` |
| Native turn lifetime | `swift test --package-path app --filter TurnLifecycleTests` |
| Complete native app, including bundle checks | `make app-test` |
| README screenshots, GIFs, and videos | `make media` |

Use `cargo fmt --all` to apply Rust formatting. Follow nearby Swift conventions and `.editorconfig`;
do not reformat unrelated files. Rust-only contributors on Linux should follow the dependencies
in [CI](.github/workflows/ci.yml) and run the Rust checks above. Native app checks still need a Mac.

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
items as not applicable with a short reason; documentation-only changes do not need a native rebuild.

## Review and maintenance

**Lead maintainer: Mohamed Alkiswani** ([mk@getoya.ai](mailto:mk@getoya.ai)).
Keel is sponsored by [getOya.ai](https://getoya.ai), a runtime for accurate agents.

Maintainers decide scope and merge pull requests. A passing test suite is necessary for code
changes but is not automatic approval. Review favors user-work safety, native UX, understandable
behavior, and compatibility with existing Claude sessions. Disagreements should focus on evidence
and trade-offs; explain a concern in the PR or related issue.

Please give reviewers time. Maintainers may ask to split a change, request tests, or decline a
feature that does not fit the product. There is no guaranteed review or support response time.

Release publication is maintainer-only. Do not run `make release` for a contribution: it changes
versions, creates a commit, pushes code, and tags a release. Signing and publishing credentials
never belong in a pull request.

## License and orientation

Submit only code and assets you have the right to contribute under the project's [MIT license](LICENSE).
Keep applicable copyright notices and identify third-party code or assets and their licenses.

Review [Privacy](docs/privacy.md) before adding telemetry or diagnostics. Never attach prompts,
repository names, private paths, or source content to reporting events.

[Architecture](docs/architecture.md) explains the modules. [Guardrails](docs/guardrails.md) describes
runtime protections. The [launch kit](docs/launch.md) explains how to regenerate product media.
