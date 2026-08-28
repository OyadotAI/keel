#!/usr/bin/env bash
# Wrap dist/Keel.app in a drag-to-Applications disk image.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"
[ -d dist/Keel.app ] || { echo "dist/Keel.app is missing — run make app first" >&2; exit 1; }

# The version is inherited from the workspace, so it is read from there rather than from the
# crate, where the field says `version.workspace = true` and a naive grep finds nothing.
version="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"
stage="$(mktemp -d)"
trap 'rm -rf "$stage"' EXIT

cp -R dist/Keel.app "$stage/"
ln -s /Applications "$stage/Applications"

# A plain read-only image. Background art and window geometry need AppleScript driving Finder,
# which fails on headless machines and in CI — not worth the decoration.
rm -f dist/Keel.dmg
hdiutil create -quiet -volname "Keel $version" -srcfolder "$stage" \
  -ov -format UDZO dist/Keel.dmg

identity="${KEEL_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null \
  | grep -o '"Developer ID Application: [^"]*"' | head -1 | tr -d '"')}"

if [ -n "$identity" ]; then
  echo "==> signing the image"
  codesign --force --sign "$identity" --timestamp dist/Keel.dmg
fi

# Notarisation needs credentials stored in the keychain, which `notarytool store-credentials` asks
# for interactively — an Apple ID, a team id and an app-specific password. It cannot be driven from
# here, so the image is built either way and the command to enable it is printed rather than
# assumed.
profile="${KEEL_NOTARY_PROFILE:-keel}"
if [ -n "$identity" ] && xcrun notarytool history --keychain-profile "$profile" >/dev/null 2>&1; then
  echo "==> notarising (a few minutes)"
  xcrun notarytool submit dist/Keel.dmg --keychain-profile "$profile" --wait
  xcrun stapler staple dist/Keel.dmg
  echo "    stapled — this opens with no warning on any Mac"
else
  echo
  echo "    Not notarised. It is signed, but Gatekeeper still blocks a first open."
  echo "    One-time setup, then re-run this:"
  echo "      xcrun notarytool store-credentials $profile \\"
  echo "        --apple-id <your-apple-id> --team-id <team> --password <app-specific-password>"
fi

echo
echo "    dist/Keel.dmg"
spctl --assess --type open --context context:primary-signature -v dist/Keel.dmg 2>&1 | sed 's/^/    /' || true
