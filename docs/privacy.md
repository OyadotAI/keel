# Privacy and local data

Keel is a local macOS app, not an offline model. This page describes the current code and build
configuration, including what can leave your Mac.

## Agent providers and developer tools

Keel drives the provider CLI installed on your Mac. Claude Code uses your Claude authentication;
prompts, attachments, repository context, and tool results needed for a task can be sent to the
provider under that account's terms. Other configured providers and tools have their own data flows.
Keel does not make those requests private merely by showing them in a native window.

Installing a CLI, plugin, MCP server, or development dependency contacts its download service and
can execute third-party code. Review installation commands and the tools you authorize.

## Crash reports and usage events

Reporting depends on the build. `KEEL_SENTRY_DSN`, `KEEL_POSTHOG_KEY`, and `KEEL_POSTHOG_HOST`
are explicit build inputs. Builds without reporting keys leave the corresponding SDK off.
The build script does not borrow configuration from neighboring projects.

When configured, crash reporting and usage reporting are **enabled by default**. Disable either
independently in **Settings → Privacy**. Turning reporting off stops future reporting through that
switch; it does not delete reports already delivered.

App-launched daemons do not initialize a separate reporting client, so they cannot bypass that
switch. A daemon manually launched with `keel serve --sentry-dsn` is a separate, explicit opt-in
and is not controlled by the app's settings. Restart older app/daemon processes after upgrading
to apply this change; changing files on disk does not stop an already-running older reporter.

- **Sentry:** crash diagnostics, app/version tags, UI breadcrumbs, and selected failure events.
  The app disables default PII collection and performance tracing. Its failure-message helper
  redacts paths, URLs, and quoted fragments. Crash diagnostics can still describe the runtime,
  operating system, stack frames, and loaded modules; do not treat them as guaranteed anonymous.
- **PostHog:** explicit usage events with operation names, counts, outcomes, app version, and OS
  information. Automatic screen-view and application-lifecycle capture are disabled. The SDK can
  use a persistent installation identifier; pseudonymous usage is not the same as anonymity.
- **Network metadata:** reporting services can receive ordinary connection metadata, including
  the source IP address. Provider-side retention and processing depend on the service configuration.

Reporting call sites are intended for operation metadata, not prompts, source files, or repository
names. Contributors must preserve that boundary and review any new event properties.

## Data kept on this Mac

- Existing Claude transcripts remain in Claude's local storage. Keel reads discoverable history
  and follows transcript updates; it does not provide cloud history synchronization.
- Keel preferences, pairing records, and project authorizations live under `~/.keel` by default.
  Trust and approved command rules live in private `~/.keel/permissions/` records, outside the
  repository. Test/development daemons can set an absolute `KEEL_PERMISSIONS_DIR` instead.
- Project-local `.keel` data can include attachments, session facts, worktrees, ignored findings,
  and quarantined configuration. Keep these out of public commits unless deliberately reviewed.
- Repository-supplied `.keel/permissions.json` files are ignored. They are not proof that this
  user authorized the repository. After updating from an older version, reapprove trust or rules
  in Settings → Permissions. Keel leaves the old files intact for manual review.
- GitHub and Cloudflare connections use the OS keychain where implemented. Claude account
  authentication remains the provider CLI's responsibility.

## Network access to the daemon

Loopback is the default. Optional pairing can enable LAN or Tailscale access, authenticated with
paired-device bearer tokens. A plain LAN connection is not an encrypted transport; prefer
loopback or a trusted encrypted tunnel. Pairing grants access to a powerful developer tool,
not a read-only dashboard. Do not expose the daemon directly to the public internet.

## Sharing diagnostics and questions

Screenshots, exported sessions, copied crash reports, and issue attachments can contain private
paths or code. Review and redact them before sharing. Do not upload credentials or complete
private transcripts to public issues.

Contact Mohamed Alkiswani at [mk@getoya.ai](mailto:mk@getoya.ai) for privacy questions or requests
about reports already submitted. For security vulnerabilities, use the [private reporting
instructions](../SECURITY.md).
