#!/bin/sh
# Stage the daemon where Tauri's `externalBin` looks for it: desktop/src-tauri/binaries/keel-<triple>.
# The app ships `keel` beside its own executable and starts one per project (main.rs `keel_binary`).
#
#   packaging/sidecar.sh                      the debug build, for this machine (make dev)
#   packaging/sidecar.sh release <triple>     a release build for one target
#   packaging/sidecar.sh release universal    both macOS architectures, joined with lipo
set -eu
profile="${1:-debug}"
triple="${2:-$(rustc -vV | sed -n 's/^host: //p')}"
root="$(cd "$(dirname "$0")/.." && pwd)"
out="$root/desktop/src-tauri/binaries"
mkdir -p "$out"
flag=""; [ "$profile" = release ] && flag="--release"
ext=""; case "$triple" in *windows*) ext=".exe" ;; esac

if [ "$triple" = universal ]; then
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cargo build -p keel $flag --target "$t" --manifest-path "$root/Cargo.toml"
  done
  lipo -create -output "$out/keel-universal-apple-darwin" \
    "$root/target/aarch64-apple-darwin/$profile/keel" "$root/target/x86_64-apple-darwin/$profile/keel"
  # Each architecture as well: `tauri build --target universal-apple-darwin` builds the app once
  # per architecture, and each of those builds looks for its own triple's file — the release
  # failed on `binaries/keel-aarch64-apple-darwin doesn't exist` with only the universal one here.
  for t in aarch64-apple-darwin x86_64-apple-darwin; do
    cp "$root/target/$t/$profile/keel" "$out/keel-$t"
  done
  exit 0
fi

if [ -n "${2:-}" ]; then
  cargo build -p keel $flag --target "$triple" --manifest-path "$root/Cargo.toml"
  src="$root/target/$triple/$profile/keel$ext"
else
  cargo build -p keel $flag --manifest-path "$root/Cargo.toml"
  src="$root/target/$profile/keel$ext"
fi
cp "$src" "$out/keel-$triple$ext"
