# Packaging

Keel is a Tauri app (`desktop/`) with the Rust daemon (`keel`) bundled inside it, one daemon per
open project. A release is a universal macOS app (DMG plus an updater archive) and a Windows NSIS
installer, published to the public `OyadotAI/keel-releases` repository. Installed copies update
themselves from there.

## Building locally

    packaging/sidecar.sh release aarch64-apple-darwin
    cd desktop && pnpm tauri build --target aarch64-apple-darwin --bundles app,dmg

`sidecar.sh` stages the daemon where `externalBin` expects it, at
`desktop/src-tauri/binaries/keel-<triple>`. Tauri puts it beside the app's executable, and
`keel_binary()` in `desktop/src-tauri/src/main.rs` finds it there. Without the staged file Tauri
refuses to build at all, and that includes `tauri dev`, which is why `make dev` runs it first.

A build is only signed if `APPLE_SIGNING_IDENTITY` names a Developer ID in the keychain. An
unsigned build is for your own machine and nothing else.

The macOS signing entitlements include `com.apple.security.cs.allow-jit` for the bundled
Node runtime. Without it, hardened-runtime builds can crash in V8 initialization before the
agent helper starts. After signing, run `node desktop/scripts/runtime-smoke.mjs /path/to/Keel.app`.
This starts the shipped runtime and checks that the helper processes a command; `--version`
alone does not exercise V8. The release workflow runs this check before publishing.

## Updates

The app checks `releases/latest/download/latest.json` at launch and every six hours, downloads in
the background, and installs only when the person clicks **Update now** in the header. A restart stops
every agent the app is running, so Keel never makes that choice for them, and it says how many
lanes are mid-turn before it restarts. Development builds never check.

Updates are signed with an updater key that has nothing to do with Apple. The public half is in
`tauri.conf.json`. The private half is created once:

    make updater-keys        # writes ~/.tauri/keel-updater.key, prints the secret command

## Swift testers

Builds of the Swift app update through Sparkle from `appcast.xml` on the same feed. The Tauri app
carries the Swift app's bundle identifier (`ai.oya.keel`) and is signed with the same Team ID, so
Sparkle accepts it as the next version and installs it in place. The release workflow therefore
also writes a Sparkle appcast for the DMG, using the old Sparkle key. After that first update the
Tauri updater takes over, and nobody has to download anything by hand. Keep publishing the
appcast until no Swift build is left in use.

## Releasing

`make release` bumps the patch version (0.3.2 → 0.3.3) in `Cargo.toml` and
`desktop/src-tauri/Cargo.toml` together, commits it, and pushes a `v*` tag.
`make release VERSION=0.4.0` sets the version instead. A version that is already tagged is refused
before anything is committed or pushed. `.github/workflows/release.yml` does the rest:

1. `draft` opens a draft release on the feed.
2. `macos` builds the universal app, signs and notarises it, and staples the DMG.
3. `windows` builds the installer.
4. `publish` writes `latest.json` from the signatures, publishes it, and mirrors the installers
   to this repository.

The workflow needs these repository secrets:

| Secret | What it is |
| --- | --- |
| `MACOS_CERT_P12` | The Developer ID Application certificate and key, `base64 -i cert.p12` |
| `MACOS_CERT_PASSWORD` | The password set when exporting that `.p12` |
| `MACOS_SIGN_IDENTITY` | The identity's name, e.g. `Developer ID Application: Oya (TEAMID)` |
| `APPLE_ID`, `APPLE_TEAM_ID`, `APPLE_APP_PASSWORD` | For notarisation |
| `TAURI_SIGNING_PRIVATE_KEY` | `~/.tauri/keel-updater.key`, from `make updater-keys` |
| `TAURI_SIGNING_PRIVATE_KEY_PASSWORD` | Empty unless the key was made with one |
| `SPARKLE_PRIVATE_KEY` | The Swift app's Sparkle key, for the migration appcast above |
| `RELEASES_TOKEN` | A token with `contents: write` on `OyadotAI/keel-releases`, since the default token cannot reach another repository |

**Windows is not Authenticode-signed yet.** There is no certificate, so SmartScreen warns on
first run. Updates are still verified by the updater's own signature before they install.

## Order matters

Tauri notarises the app. The image it is put in is a second file Gatekeeper checks, so the
workflow notarises and staples the DMG as well. If only the app is stapled, the image passes on
every machine with a network and fails on one without.

## A 403 about agreements

    HTTP status code: 403. A required agreement is missing or has expired.

Two separate places, and accepting one does not clear the other. `notarytool` authenticates against
the App Store Connect API, so **appstoreconnect.apple.com → Business** is the one usually
outstanding. It is not the developer portal, which is where everyone looks first. Only the Account
Holder can accept either. Nothing about the certificate or the build is involved; signing keeps
working throughout.
