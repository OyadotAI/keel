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

echo "==> building the Keel app (release)"
swift build --package-path app -c release

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
sed "s/__VERSION__/$version/g" packaging/Info.plist > "$app/Contents/Info.plist"

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
