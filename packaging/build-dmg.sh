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

echo "    dist/Keel.dmg"
