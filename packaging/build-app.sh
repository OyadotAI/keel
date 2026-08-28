#!/usr/bin/env bash
# Build dist/Keel.app from a release binary.
#
# There is no Xcode project because there is nothing to compile that cargo does not already build:
# a bundle is a directory with a plist in it.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

# The version is inherited from the workspace, so it is read from there rather than from the
# crate, where the field says `version.workspace = true` and a naive grep finds nothing.
version="$(sed -n 's/^version *= *"\(.*\)"/\1/p' Cargo.toml | head -1)"
app="dist/Keel.app"

echo "==> building keel $version (release)"
cargo build --release --quiet

echo "==> assembling $app"
rm -rf "$app"
mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"
cp target/release/keel "$app/Contents/MacOS/keel"
sed "s/__VERSION__/$version/g" packaging/Info.plist > "$app/Contents/Info.plist"

echo "==> rendering the icon"
python3 packaging/icon.py dist/icon >/dev/null
iconutil -c icns dist/icon/Keel.iconset -o "$app/Contents/Resources/Keel.icns"

# Ad-hoc, so it runs on this machine. Distribution needs a Developer ID — see packaging/README.md.
echo "==> signing (ad-hoc)"
codesign --force --deep --sign - "$app"
codesign --verify --strict "$app" && echo "    signature verifies"

echo
echo "    $app"
echo "    open it, or: cp -R $app /Applications/"
