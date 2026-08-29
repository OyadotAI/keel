#!/usr/bin/env bash
# Build dist/Keel.app from a release binary.
#
# There is no Xcode project. The app is Swift built with SwiftPM and the daemon is built by cargo;
# a bundle is a directory with a plist in it, and neither toolchain needs Xcode's project format to
# produce one. `xcodebuild` also refuses to run until its licence is accepted, which is a thing to
# make a release depend on only if it buys something.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# The version is inherited from the workspace, so it is read from there rather than from the
# crate, where the field says `version.workspace = true` and a naive grep finds nothing.
version="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"
app="dist/Keel.app"

echo "==> building the keel daemon $version (release)"
cargo build --release --quiet

# Reporting keys and the update key, from the environment or a gitignored .env beside this
# script's repo. Empty is fine: the SDK stays off and the updater never starts.
[ -f .env ] && set -a && . ./.env && set +a
: "${KEEL_SENTRY_DSN:=}"
: "${KEEL_POSTHOG_KEY:=}"
: "${KEEL_POSTHOG_HOST:=}"
: "${KEEL_SPARKLE_PUBLIC_KEY:=}"
# The PostHog project is shared with A2ABase, so its public client key is read from there
# when this repo does not set one. Events are told apart by the `app=keel` property.
if [ -z "$KEEL_POSTHOG_KEY" ] && [ -f ../A2ABaseAI/frontend/.env.local ]; then
  KEEL_POSTHOG_KEY="$(sed -n 's/^NEXT_PUBLIC_POSTHOG_KEY=//p' ../A2ABaseAI/frontend/.env.local | tr -d '"' | head -1)"
  KEEL_POSTHOG_HOST="$(sed -n 's/^NEXT_PUBLIC_POSTHOG_HOST=//p' ../A2ABaseAI/frontend/.env.local | tr -d '"' | head -1)"
fi
if [ -z "$KEEL_SPARKLE_PUBLIC_KEY" ] && command -v packaging/sparkle/bin/generate_keys >/dev/null 2>&1; then
  KEEL_SPARKLE_PUBLIC_KEY="$(packaging/sparkle/bin/generate_keys -p 2>/dev/null || true)"
fi
echo "    sentry: $([ -n "$KEEL_SENTRY_DSN" ] && echo on || echo off)"
echo "    posthog: $([ -n "$KEEL_POSTHOG_KEY" ] && echo on || echo off)"
echo "    updates: $([ -n "$KEEL_SPARKLE_PUBLIC_KEY" ] && echo on || echo off)"

echo "==> building the Keel app (release)"
# The rpath is for Sparkle, which is a dynamic framework and rides in Contents/Frameworks.
swift build --package-path app -c release -Xlinker -rpath -Xlinker @executable_path/../Frameworks

echo "==> assembling $app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
# The Swift app is what launches; the Rust binary rides along beside it as the daemon the app
# spawns, and is the same binary as `keel serve` on a PATH. One artifact, two entry points.
cp "app/$(swift build --package-path app -c release --show-bin-path)/KeelApp" \
   "$app/Contents/MacOS/KeelApp" 2>/dev/null \
  || cp "$(swift build --package-path app -c release --show-bin-path)/KeelApp" \
        "$app/Contents/MacOS/KeelApp"
cp target/release/keel "$app/Contents/MacOS/keel"
bin="$(swift build --package-path app -c release --show-bin-path)"
# SwiftPM resources. `Bundle.module` wants KeelApp_KeelApp.bundle beside the executable and
# traps without it; the app looks in Contents/Resources first (Resources.swift), so the files
# go there — and the bundle itself rides along for anything that still asks by that name.
if [ -d "$bin/KeelApp_KeelApp.bundle" ]; then
  cp -R "$bin/KeelApp_KeelApp.bundle/." "$app/Contents/Resources/"
  cp -R "$bin/KeelApp_KeelApp.bundle" "$app/Contents/Resources/"
fi
if [ -d "$bin/Sparkle.framework" ]; then
  mkdir -p "$app/Contents/Frameworks"
  cp -R "$bin/Sparkle.framework" "$app/Contents/Frameworks/"
fi
sed -e "s/__VERSION__/$version/g" \
    -e "s|__SENTRY_DSN__|$KEEL_SENTRY_DSN|g" \
    -e "s|__POSTHOG_KEY__|$KEEL_POSTHOG_KEY|g" \
    -e "s|__POSTHOG_HOST__|$KEEL_POSTHOG_HOST|g" \
    -e "s|__SU_PUBLIC_KEY__|$KEEL_SPARKLE_PUBLIC_KEY|g" \
    packaging/Info.plist > "$app/Contents/Info.plist"

echo "==> rendering the icon"
python3 packaging/icon.py dist/icon >/dev/null
iconutil -c icns dist/icon/Keel.iconset -o "$app/Contents/Resources/Keel.icns"

# Sign with a Developer ID when one is on this machine, ad-hoc when not.
#
# Ad-hoc is enough to run where it was built and nowhere else: Gatekeeper tells anybody else the
# app "is damaged and can't be opened", which is what it says instead of the truth. A Developer ID
# plus notarisation is the only thing that opens cleanly on someone else's Mac.
identity="${KEEL_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null \
  | grep -o '"Developer ID Application: [^"]*"' | head -1 | tr -d '"')}"

if [ -n "$identity" ]; then
  echo "==> signing as $identity"
  # --options runtime is the hardened runtime, which notarisation requires and will not explain
  # the absence of. --timestamp is likewise mandatory and likewise silent when missing.
  codesign --force --deep --options runtime --timestamp \
    --sign "$identity" "$app"
else
  echo "==> signing (ad-hoc — no Developer ID found; this will not open on another Mac)"
  codesign --force --deep --sign - "$app"
fi
codesign --verify --deep --strict "$app" && echo "    signature verifies"

echo
echo "    $app"
echo "    open it, or: cp -R $app /Applications/"
