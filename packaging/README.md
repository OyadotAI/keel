# Packaging

`make app` builds `dist/Keel.app`; `make dmg` wraps it in `dist/Keel.dmg` with a drag-to-Applications
alias. Both need only a Mac and a release build — no Xcode project, no signing certificate.

## What the bundle is

The app shell is SwiftUI/AppKit, built with SwiftPM. `Contents/MacOS/KeelApp` launches the UI;
`Contents/MacOS/keel` is the bundled Rust daemon. The app communicates with the daemon over
loopback for agent runs, Git operations, terminals, and session updates. WebKit is used for
in-app web previews, not to render the application shell.

The bundle also contains SwiftPM resources, the generated icon, and Sparkle.framework. No Xcode
project is needed, but building the current Swift code requires an Xcode 26+ toolchain. The app's
deployment target remains macOS 15.

## Local and CI builds

Install Pillow in a virtual environment before `make app` or `make app-test`; see
[Contributing](../CONTRIBUTING.md). `KEEL_SIGN_IDENTITY=- make app-test` explicitly uses ad-hoc
signing for a local/CI build, without a Developer ID, notarization credentials, or release secrets.

Optional reporting keys are read only from the build environment or this repository's ignored
`.env`. Leaving them unset disables reporting in that build. See [Privacy](../docs/privacy.md).
Never commit `.env`, signing certificates, or update private keys. The Sparkle public verification
key is public by design and is distinct from the signing key.

## Signing

`make app` and `make dmg` sign with a Developer ID when one is in the keychain, found automatically
or named by `KEEL_SIGN_IDENTITY`. Both use the hardened runtime and a secure timestamp, which
notarisation requires and does not explain the absence of.

Signed is not enough — Gatekeeper says so exactly:

    dist/Keel.app: rejected
    source=Unnotarized Developer ID

Notarisation closes it, and needs credentials in the keychain. `notarytool store-credentials` asks
for them interactively — an Apple ID, a team id, an app-specific password — so it cannot be driven
from a script. Once, and every `make dmg` notarises and staples on its own:

    xcrun notarytool store-credentials keel \
      --apple-id <apple-id> --team-id <team> --password <app-specific-password>

Use `KEEL_NOTARY_PROFILE` for a different profile name.

## Order matters

The app is notarised and stapled **before** the image is built from it, and the image is notarised
after. Doing only the image leaves the app inside without a ticket — Gatekeeper still accepts it,
by asking Apple, so it passes on every machine you would test on and fails on one with no network.

That is not hypothetical. An app copied out of an image notarised on its own reported this, in the
same breath:

    accepted
    source=Notarized Developer ID
    Keel.app does not have a ticket stapled to it.

## Releasing

`make release` bumps the version in `Cargo.toml`, commits it, and pushes a `v*` tag.
`.github/workflows/release.yml` does everything after that — build, sign, notarise, staple, write
the appcast, publish to `OyadotAI/keel-releases`. Nothing about a release depends on which Mac you
are sitting at, which was the point.

The workflow needs these repository secrets, and does nothing useful without them:

| Secret | What it is |
| --- | --- |
| `MACOS_CERT_P12` | The Developer ID Application certificate and key, `base64 -i cert.p12` |
| `MACOS_CERT_PASSWORD` | The password set when exporting that `.p12` |
| `MACOS_SIGN_IDENTITY` | Optional. Left empty, the job finds the identity it just imported |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | The three `notarytool store-credentials` asks for |
| `SPARKLE_PRIVATE_KEY` | `packaging/sparkle/bin/generate_keys -x -` on the machine that ran `make sparkle-keys` |
| `SPARKLE_PUBLIC_KEY` | The public half, baked into `Info.plist` so the updater trusts the feed |
| `RELEASES_TOKEN` | A token with `contents: write` on `OyadotAI/keel-releases` — the default one cannot reach another repository |
| `KEEL_SENTRY_DSN`, `KEEL_POSTHOG_KEY`, `KEEL_POSTHOG_HOST` | Optional. Absent, that SDK is off in the build |

If the workflow is down, the manual path is what it always was — `make dmg`, then upload the two
files it leaves in `dist/`:

    gh release create v0.2.52 dist/Keel.dmg dist/appcast.xml -R OyadotAI/keel-releases --latest

## If notarisation is not set up

Use the build for local development only. Ad-hoc or unnotarized artifacts are not suitable for
public distribution and can be rejected by Gatekeeper. Finish signing and notarization before
publishing a download; do not ask users to bypass quarantine as the normal installation path.

## A 403 about agreements

    HTTP status code: 403. A required agreement is missing or has expired.

Two separate places, and accepting one does not clear the other. `notarytool` authenticates against
the App Store Connect API, so **appstoreconnect.apple.com → Business** is the one usually
outstanding — not the developer portal, which is where everyone looks first. Only the Account
Holder can accept either. Nothing about the certificate or the build is involved; signing keeps
working throughout.
