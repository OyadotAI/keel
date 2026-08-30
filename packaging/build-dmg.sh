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

identity="${KEEL_SIGN_IDENTITY:-$(security find-identity -v -p codesigning 2>/dev/null \
  | grep -o '"Developer ID Application: [^"]*"' | head -1 | tr -d '"')}"
profile="${KEEL_NOTARY_PROFILE:-keel}"
notarise=false
if [ -n "$identity" ] && xcrun notarytool history --keychain-profile "$profile" >/dev/null 2>&1; then
  notarise=true
fi

# The app is notarised and stapled *before* the image is built from it.
#
# Notarising only the image leaves the app inside it without a ticket. Gatekeeper still accepts it,
# by asking Apple — so it passes on the machine you tested on and fails on a laptop with no
# network, which is the worst possible way to find out. Verified: the app copied out of an image
# notarised on its own reported "accepted / Notarized Developer ID" and, in the same breath, "does
# not have a ticket stapled to it".
if $notarise; then
  echo "==> notarising the app (a few minutes)"
  ditto -c -k --keepParent dist/Keel.app "$stage/Keel-notarize.zip"
  xcrun notarytool submit "$stage/Keel-notarize.zip" --keychain-profile "$profile" --wait
  xcrun stapler staple dist/Keel.app
  rm -f "$stage/Keel-notarize.zip"
fi

cp -R dist/Keel.app "$stage/"
ln -s /Applications "$stage/Applications"

# A plain read-only image. Background art and window geometry need AppleScript driving Finder,
# which fails on headless machines and in CI — not worth the decoration.
rm -f dist/Keel.dmg
hdiutil create -quiet -volname "Keel $version" -srcfolder "$stage" \
  -ov -format UDZO dist/Keel.dmg

if [ -n "$identity" ]; then
  echo "==> signing the image"
  codesign --force --sign "$identity" --timestamp dist/Keel.dmg
fi

# Notarisation needs credentials stored in the keychain, which `notarytool store-credentials` asks
# for interactively — an Apple ID, a team id and an app-specific password. It cannot be driven from
# here, so the image is built either way and the command to enable it is printed rather than
# assumed.
if $notarise; then
  echo "==> notarising the image"
  xcrun notarytool submit dist/Keel.dmg --keychain-profile "$profile" --wait
  xcrun stapler staple dist/Keel.dmg
  echo "    stapled — both the image and the app inside it, so this opens offline too"
else
  echo
  echo "    Not notarised. It is signed, but Gatekeeper still blocks a first open."
  echo "    One-time setup, then re-run this:"
  echo "      xcrun notarytool store-credentials $profile \\"
  echo "        --apple-id <your-apple-id> --team-id <team> --password <app-specific-password>"
fi

# The appcast Sparkle reads. `generate_appcast` signs the image with the EdDSA key in the
# keychain and writes appcast.xml beside it; the release workflow uploads both to the GitHub
# release the feed URL points at.
#
# A CI runner has no keychain of ours, so `KEEL_SPARKLE_KEY_FILE` points at the key on disk
# instead. Set via `set --` rather than an array: /bin/bash on macOS is 3.2, where an empty
# array expands badly under `set -u`.
if [ -x packaging/sparkle/bin/generate_appcast ]; then
  echo "==> writing the appcast"
  rm -f dist/appcast.xml
  if [ -n "${KEEL_SPARKLE_KEY_FILE:-}" ]; then
    set -- --ed-key-file "$KEEL_SPARKLE_KEY_FILE"
  else
    set --
  fi
  packaging/sparkle/bin/generate_appcast "$@" \
    --download-url-prefix "https://github.com/OyadotAI/keel-releases/releases/download/v$version/" \
    -o dist/appcast.xml dist/ >/dev/null
  echo "    dist/appcast.xml"
fi

echo
echo "    dist/Keel.dmg"
spctl --assess --type open --context context:primary-signature -v dist/Keel.dmg 2>&1 | sed 's/^/    /' || true
# The app is what a user ends up running, so its own state is what is worth reporting.
xcrun stapler validate dist/Keel.app 2>&1 | tail -1 | sed 's/^/    app: /'
