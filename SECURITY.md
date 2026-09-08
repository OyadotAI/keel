# Security policy

Keel runs local tools and can read or modify repositories. Reports involving command execution,
permission bypasses, credential exposure, unauthorized daemon access, unsafe Git operations, or
malicious project configuration deserve private handling.

## Report privately

Email lead maintainer **Mohamed Alkiswani** at **[mk@getoya.ai](mailto:mk@getoya.ai)**.
Use a subject such as `Keel security report`.
If GitHub's **Report a vulnerability** button is available under this repository's Security tab,
you may use that private channel instead. See [GitHub's private-reporting guide](https://docs.github.com/en/code-security/how-tos/report-and-fix-vulnerabilities/report-privately).

Do not open a public issue or pull request with exploit details, secrets, or someone else's
repository data. Do not test against other users, production services, or repositories you do not own
without their authorization.

Include what you safely can:

- Keel version or commit, macOS version, and relevant Claude Code/tool versions.
- The affected feature and the boundary you believe was crossed.
- Minimal reproduction steps using a disposable repository and synthetic data.
- Expected versus actual behavior, impact, and any proposed fix.

Redact tokens, session cookies, passwords, personal paths, account identifiers, and private code.
Do not email entire Claude transcripts or keychain exports. If credentials were exposed, revoke
or rotate them with the issuing provider; removing a screenshot or log does not revoke a credential.

## Handling and fixes

Maintainers will assess the report, ask for clarification when needed, and coordinate a fix and
disclosure with the reporter. We aim to acknowledge reports promptly, but do not promise a response
deadline, a bounty, or a fix date. Ask before publishing an unfixed exploit; once a fix is available,
coordinate a useful public explanation that does not expose user data.

Security work targets current `main` and the latest published release. Older releases do not
have a separate long-term security-maintenance commitment. Include an older affected version in
your report anyway; do not take risks reproducing a destructive issue on a working repository.

## Boundaries

The app talks to a local daemon, but configured providers receive the prompts and repository
context needed for agent requests. Running locally does not make those requests offline.
Installed tools and repository-supplied hooks or plugins can execute code and need careful review.

See [Guardrails](docs/guardrails.md) for implemented protections and their limits, and
[Support](SUPPORT.md) for ordinary setup and bug reports.
